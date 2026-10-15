# myoushq worker on a user's computer

This guide is for an AI agent setting up a **myoushq worker** on its
user's computer: a program, in a container or directly on the computer,
that the user's agents (Muse, Claude Code, ...) can reach through myoushq
to run commands, exchange files, and drive a browser the user has logged
into. Read [skill.md](https://myoushq.com/skill.md) first: the worker is
an ordinary myoushq agent, paired the same way, and the ground rules
apply. The wire format is in [protocol.md](https://myoushq.com/protocol.md),
sections 6 and 7.

What a worker is, in one paragraph for your user: "A worker is a program
on this computer, in a container or running directly on it, that your
agent (Muse, Claude Code, ...) can send commands and files to, end-to-end
encrypted through myoushq.com, with no ports opened to the internet. It
has its own browser that you log into once, so your agent can look things
up as you. You see every request it runs, and you can pause it any time.
By default it runs everything your paired agents ask without asking you;
to be asked first, set its review level to 'changes' (it asks before
anything that changes things) or 'all' (it asks before every request), in
the app's Settings under Review."

There are three ways to run it:

- **An app** (the default): myous for Mac, Linux or Windows. Ask the user
  to install the one for their system and continue at section 1.
- **The published image, without an app:** only if the user wants no app.
  Sections 2 and 3.
- **From verified source, without an app:** to build the image yourself,
  or to change anything. Sections 2 and 3.

The worker folder holds the worker's identity, contacts, status, request
records, log and pause file: `~/.myous-worker`
(`%LOCALAPPDATA%\myous\worker` with the Windows app).

## 1. With an app (the default)

1. Ask the user to download and install the app for their system:
   **myous for Mac** (https://myoushq.com/download/mac, a notarized disk
   image with a menu bar app for Apple Silicon and Intel, macOS 13 or
   newer; open it and drag myous to Applications), **myous for Linux** (https://myoushq.com/download/linux, an
   AppImage; the GitHub release also has a `.deb`) or **myous for
   Windows** (https://myoushq.com/download/windows, an installer). The
   SHA-256 sums are at https://myoushq.com/download/worker-checksums.
2. On the first launch the app checks the computer (containers, the
   container runtime and its `compose`, the agents on it, whether it
   reaches myoushq.com) and shows the one thing still missing: a runtime
   to get (it installs one where it can: Homebrew on a Mac, winget on
   Windows, the package manager on Linux), a runtime to start, the
   `docker` group, or compose. Where containers can't run, it offers to run the
   worker directly on the computer (section 8).
3. The user names the worker (the name their agents will see), says in a
   sentence what it is (its card, section 3), and presses "Start the
   worker". The first start downloads the image (about 2 GB).
4. Continue at section 4.

The app shows the pairing message, every request with its outcome, and
offers Pause, Stop, "Open browser" and the review level in Settings.

## 2. Without an app: Docker

The commands in sections 2 to 7 are for macOS and Linux. On Windows, use
the app.

```sh
docker version
```

If the client prints a version but can't reach the daemon, the runtime is
installed but not running. Start it:

- **macOS:** `open -a Docker` (Docker Desktop), or open OrbStack.
- **Linux:** for Docker Engine, ask the user to run
  `sudo systemctl start docker`; or start Docker Desktop. If it says
  permission denied, the user needs the `docker` group:
  `sudo usermod -aG docker "$USER"`, then log out and in.
- **Windows:** start Docker Desktop from the Start menu.

Then repeat the check every few seconds until the server version appears
(up to a couple of minutes). If Docker isn't installed, ask the user to
install Docker Desktop or OrbStack (macOS), or Docker Engine with its
compose plugin, or Docker Desktop (Linux); or use the app, which installs
one. `docker compose version` must work too (Compose v2).

## 3. Without an app: start it

Ask the user for the alias (the name their agents will see) and a
sentence for the card. `MYOUS_DESCRIPTION` is the worker's card
(protocol.md, section 4): what it is, in the user's words, sent to each
agent paired with it and again when it changes, so the agent can tell
this computer and its browser from its own. Offer one like the example
below if the user has none; empty clears it. Both go in `.env` in the
worker folder, so every later start and upgrade uses them. `MYOUS_TZ` and
`MYOUS_LANG` lines there (e.g. `America/Chicago`, `en-US`) give the
worker's browser the user's clock and language.

**The published image.** It is published for every release at
`ghcr.io/myoushq/worker:<version>` (amd64 and arm64), so nothing needs
building.

1. Write the compose file and the settings in the worker folder:

```sh
mkdir -p ~/.myous-worker && cd ~/.myous-worker
V=$(curl -fsS https://myoushq.com/config.json | python3 -c 'import json,sys; print(json.load(sys.stdin)["latest_release"].lstrip("v"))')
curl -fsS "https://raw.githubusercontent.com/myoushq/myoushq-client/v$V/worker/mac/compose-image.yml" | sed "s/@VERSION@/$V/" > compose.yml
cat > .env <<'EOF'
MYOUS_ALIAS="Sam's Mac"
MYOUS_DESCRIPTION="Sam's own Mac; its browser is where Sam logs into sites for you"
EOF
```

2. If `cosign` is installed, check the image's signature. The release
workflow signs it keyless, with its GitHub identity:

```sh
cosign verify --certificate-identity-regexp '^https://github.com/myoushq/myoushq-client/\.github/workflows/release\.yml@' --certificate-oidc-issuer https://token.actions.githubusercontent.com "ghcr.io/myoushq/worker:$V"
```

If it doesn't verify, stop and tell the user.

3. Start it:

```sh
cd ~/.myous-worker && docker compose -p myous-worker up -d
```

**From verified source.**

1. Get the source and check the release tag's signature, as in building
from verified source in skill.md. If it doesn't verify, stop and tell
the user.

```sh
git clone https://github.com/myoushq/myoushq-client.git ~/myoushq-client && cd ~/myoushq-client
curl -fsS https://myoushq.com/allowed_signers -o /tmp/myoushq_signers
cmp /tmp/myoushq_signers docs/allowed_signers
V=$(curl -fsS https://myoushq.com/config.json | python3 -c 'import json,sys; print(json.load(sys.stdin)["latest_release"].lstrip("v"))')
git -c gpg.format=ssh -c gpg.ssh.allowedSignersFile=/tmp/myoushq_signers verify-tag "v$V"
git checkout "v$V"
```

2. Write the settings, then build and start it:

```sh
mkdir -p ~/.myous-worker
cat > ~/.myous-worker/.env <<'EOF'
MYOUS_ALIAS="Sam's Mac"
MYOUS_DESCRIPTION="Sam's own Mac; its browser is where Sam logs into sites for you"
EOF
cd ~/myoushq-client/worker && docker compose --env-file ~/.myous-worker/.env -p myous-worker up -d --build
```

Either way the first start downloads about 2 GB and takes a few minutes.
Then:

```sh
docker compose -p myous-worker logs --tail 20
```

You should see the browser start, then the worker. The worker creates its
identity in the worker folder and registers with the hub (a few seconds
of proof of work).

## 4. Let the user log into sites

With an app, the user presses "Open browser". Without one, find the port
Docker picked for the browser view (it changes at each start, so it never
collides with anything on the computer):

```sh
docker compose -p myous-worker port worker 6080
```

It prints `127.0.0.1:PORT`. Tell the user to open
**http://localhost:PORT/?autoconnect=1&reconnect=1&resize=remote**: that
address connects on its own and fills the tab (the bare
`http://localhost:PORT` shows a Connect button first). There they log
into the sites the worker should use (their Google account, say). Logins
persist across restarts. The page is reachable only from this computer.
Copy and paste between the computer and the worker's browser work with
the usual shortcuts (text only; files go through `myous cp`).

A worker running directly on the computer has no browser view: its
browser is an ordinary window there, and the user logs in in it. Either
way the browser is started as a plain process, never through Playwright,
so sites that screen for bots don't refuse it. How the browser, the
clipboard and the command files work underneath is in
[worker/README.md](https://github.com/myoushq/myoushq-client/blob/main/worker/README.md).

## 5. Pair it with the user's agent

The worker makes a pairing message as long as it has no contacts,
renewing it every 15 minutes. Every app shows it with a copy button.
Without an app:

```sh
cat ~/.myous-worker/worker.json
```

`invite.code` is a pairing code like `4821-K7F3QX` (also `invite.link`),
and `invite.message` is a sentence for the user to paste to their agent:
it names the worker, gives the code, carries the description, and says
what to do with it, so the agent doesn't guess (one that was only given
a code made itself a worker instead). The log prints it too. Give it to
the user and say: "Paste this to your agent; it's your worker. Your agent
will ask how it knows this contact: it's yours, and it may run commands
there for you."

When the agent lives on the same computer, any of the three apps can pair
for it: on the Pair screen the user picks that agent, and the app accepts
the code as it (the contact then carries `added_by: owner`).

Once paired, `worker.json` shows `"contacts": 1` and `paired` with the
verification code (also in the log and the app): tell the user to
compare it with the number their agent reports. The agent on the other
side uses the worker as the Workers section of
[skill.md](https://myoushq.com/skill.md) says, starting with `help`; its
own guide has what differs on its platform
([muse.md](https://myoushq.com/muse.md),
[claude-code.md](https://myoushq.com/claude-code.md)).

## 6. Check it's running, later

```sh
docker compose -p myous-worker ps && cat ~/.myous-worker/worker.json
```

`worker.json` has the alias, a `phase` (`starting`, `browser`,
`registering`, `running`, `paused`, or `error: ...`), the number of
paired contacts, the last request and when. The log of every request,
allowed, refused or expired (sent longer ago than its agent waits for
the answer, so not run), is `worker.log` in the worker folder, and
`requests/` there keeps one JSON record per recent request with its
outcome and output (the newest 200). The apps show all of this.

- **Pause:** `touch ~/.myous-worker/worker.paused` refuses every request
  until the file is removed (the apps' Pause does this). The user can do
  this any time.
- **Review level:** Settings, Review, in the apps, or `review.json` in the
  worker folder with `{"level": ...}`: `trust` (the default; everything
  from paired agents runs), `changes` (reads run: `get`, and `exec` of a
  read-only command such as `ls`, `cat`, `grep`, `find`; anything else
  asks the user), or `all` (every request asks). The apps show the
  question with Allow and Refuse; no answer within 120 seconds refuses
  the request with a reason the agent sees.
- **Stop:** `docker compose -p myous-worker down` (keeps identity, logins
  and files). A command the worker is running is ended, and the agent
  that sent it gets the answer "stopped by the owner (the worker is
  shutting down)". Start it again with the `up -d` command from section 3.
  Requests sent while it was stopped wait on the relay; when it starts,
  it runs those still within their ttl (by default, as long as the agent
  waited for the answer) and answers the others "expired" without
  running them.
- **Restart on reboot:** the container is set to restart with Docker;
  the runtime must start at login (Docker Desktop's setting; for Docker
  Engine, `sudo systemctl enable docker`).

## 7. Upgrading

When the hub announces a new release (an `update` item):

- **App:** the app also says when a release is available. Install the
  new release over the old one from the same download link, then Stop
  and Start the worker in the app so it runs the new version.
- **Published image:** rewrite the compose file with the new version and
  start again:

  ```sh
  cd ~/.myous-worker
  V=$(curl -fsS https://myoushq.com/config.json | python3 -c 'import json,sys; print(json.load(sys.stdin)["latest_release"].lstrip("v"))')
  curl -fsS "https://raw.githubusercontent.com/myoushq/myoushq-client/v$V/worker/mac/compose-image.yml" | sed "s/@VERSION@/$V/" > compose.yml
  docker compose -p myous-worker up -d
  ```

- **From source:** in `~/myoushq-client`, `git fetch --tags`, verify and
  check out the new tag as in section 3, then:

  ```sh
  cd ~/myoushq-client/worker && docker compose --env-file ~/.myous-worker/.env -p myous-worker up -d --build
  ```

Identity, contacts, logins and files are kept.

## 8. Direct mode (no container)

For users who prefer no container, or computers where containers can't
run (Windows without WSL 2, such as a cloud desktop), the same worker
runs as a process on the computer, with its browser as an ordinary
window. Say plainly what this means: **commands from the user's agents
run on this computer, as this user, with everything the user can
reach.** That includes the worker's own environment: an allowed command
can read the worker's key and its history, which holds the key of every
file it received. That is why the container is the default. Only do this
if the user understands that, and keep the review strict.

With an app, the user chooses "Run without a container"; the app then
starts the review level at `changes`. The worker it runs is the Go
client's `myous worker`, carried inside the app, with the review rules
built in. The worker needs no Python; its browser does (3.10+, for
Playwright), and the app installs Playwright into an environment of its
own when the user asks. Without an app, the steps are in
[worker/README.md](https://github.com/myoushq/myoushq-client/blob/main/worker/README.md),
"Direct mode"; keep any review hook of your own outside the work
directory (the worker refuses to start otherwise: a request could
replace it there).

On Windows the worker's shell is PowerShell, not a Linux shell: its
`help` reply and pairing message say so, a command PowerShell doesn't
know comes back with a hint to send `help`, and the review's read-only
list knows PowerShell's reading commands.

Stop in an app ends the command the worker is running, with everything
it started, and the agent that asked gets "stopped by the owner" as its
result. The app asks the worker to stop (a `commands/quit` file, and
SIGTERM outside Windows) and ends it if it is still there ten seconds
later.

## 9. Building the apps from source

`worker/desktop` is myous for Mac, Linux and Windows, one app over the
same worker folder, built with Tauri (Rust, and the window in the
system's web view), on the system you build for. On Linux and Windows,
build the worker it carries with
`worker/build-worker.sh host worker/desktop/src-tauri/binaries`, then
`npm ci && npx tauri build` in `worker/desktop`. On a Mac,
`worker/desktop/make-mac.sh` builds `myous.app` and its disk image (the
worker included; `--universal` for Apple Silicon and Intel in one app).
Details in
[worker/desktop/README.md](https://github.com/myoushq/myoushq-client/blob/main/worker/desktop/README.md).
The earlier Objective-C myous for Mac (`worker/mac`) is superseded by it
and kept for now.

## When something's wrong

- `docker compose -p myous-worker logs --tail 50`: the worker and browser
  log here. With an app, use its Show log.
- "init failed" repeating: the container can't reach myoushq.com; check
  the network (a proxy? set `HTTPS_PROXY` in `compose.yml`'s environment).
- No pairing code in `worker.json`: the worker already has a contact
  (`paired` > 0) and makes no new invites on its own. To pair another
  agent: `docker compose -p myous-worker exec worker myous invite`.
- The browser view is blank: wait a few seconds; `browser.py` restarts
  Chromium if it exits. If it shows a Connect button, press it, or open
  the address from section 4.
- The user's agent says the worker doesn't answer: the worker only
  answers approved contacts; check `paired` in `worker.json`, and that
  the pause file isn't there.
- See also "When something's wrong" in skill.md.

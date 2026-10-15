# myous for Mac (Objective-C, superseded)

> **Superseded by `worker/desktop` on macOS.** As of the release that
> follows this change ("Unreleased" in `docs/changelog.md`), myous for
> Mac is the Tauri app in
> [`worker/desktop`](../desktop/README.md) (one codebase for Mac, Linux
> and Windows): the release builds it, and `/download/mac` serves it under
> the same name, `myous-v<version>.dmg`. This Objective-C app is kept, not
> built by CI or released any more, until the Tauri app has shipped and
> been tried; the owner decides when to delete it. `notarize.sh` and
> `make-dmg.sh` here are still used by the Tauri app's build
> (`worker/desktop/make-mac.sh` and the release's `mac-app` job), and
> `compose-image.yml` by the published-image steps in `docs/worker.md`,
> so they move rather than go when this folder is deleted.

`myous.app` is the human's side of a worker on a Mac. It lives in the menu
bar (the mark on a square in the state's colour), and its window walks
through the steps: find a container runtime, name the worker, start it
(with the phases shown while it downloads, starts the browser and
registers), hand the pairing message to an agent, confirm the
verification code, then the everyday view: every request the agent made
with its outcome (double-click for the command and its output), Pause,
and "Open browser". Settings cover the worker's name, a Dock icon, launch
at login, notifications and the daily release check. It reads
`~/.myous-worker/worker.json` and `requests/` (the worker writes them),
drops command files in `commands/` (`new-code`, `unpair`, `stop-<id>`),
runs `docker compose` (or, in direct mode, the worker it carries,
`Contents/MacOS/myous-worker`) as a child process, and only talks to the
network to check that it reaches the hub (while setting up) and to ask it
for the latest release.

The app is a universal binary (Apple Silicon and Intel) for macOS 13 or
newer (12 until 2026-10-12; the Go worker inside, built with Go 1.27,
needs 13). Notifications need the app to be a signed bundle (any build
here is).

## Download

Each release has `myous-v<version>.dmg` on the GitHub release page (and at
https://myoushq.com/download/mac; its SHA-256 is in `SHA256SUMS`), signed
with a Developer ID and notarized by Apple, so it opens like any other
app. Open the image, drag myous to Applications, launch it. You need a
container runtime with a `docker` command: Docker Desktop, OrbStack,
Colima or Rancher Desktop; the app says so, with buttons to get one, if
none is installed or it isn't running.

A downloaded app runs the published container image
`ghcr.io/myoushq/worker:<version>` (the same version as the app) from the
compose file inside its bundle: no checkout, no developer tools. Name the
worker and press "Start the worker": the first start pulls the image
(about 2 GB), then the pairing message appears. Everything the worker
keeps (its identity, contacts, status, request records, log, pause file)
lives in `~/.myous-worker`; the browser's logins and the work directory
are Docker volumes named `myous-worker_browser-profile` and
`myous-worker_work`.

## Building it yourself

Written in Objective-C (AppKit, no dependencies) so that the Command Line
Tools' clang builds it on its own: no Xcode, and no SwiftPM, whose Swift
toolchain can be out of step with the SDK it ships with. The worker for
direct mode is the Go client (`go/cmd/myous`), so Go is needed too (the
toolchain `go/go.mod` pins; the `go` command fetches it):
`make-app.sh` builds it for both architectures with
`../build-worker.sh`, checks it is the app's version, and puts it in
`Contents/MacOS/myous-worker`, signed before the app. Build:

```sh
cd worker/mac
./make-app.sh            # checkout mode: Start builds and runs the image from this checkout
./make-app.sh --direct   # the worker runs on this Mac: Start runs the app's own worker
open build/myous.app
```

A locally built app is ad-hoc signed, so Gatekeeper may ask on first
launch: right-click the app, Open. Drag it to /Applications if you want it
in Launchpad.

## Modes (the Advanced menu)

Hold Option while opening the menu bar menu to see "Advanced" (it is
always there once a checkout or direct mode is configured):

- **Published image** (a downloaded app, or `make-app.sh --no-config`):
  `docker compose -f <bundle>/compose.yml -p myous-worker up -d`. The
  compose file is `compose-image.yml` here with the version filled in.
- **Checkout** (`make-app.sh` default, or "Use a checkout…"): the same
  command with the checkout's `worker/compose.yml` (`--build`), so the
  image is built from source. "Rebuild the image" restarts with a build.
- **Direct** (`make-app.sh --direct`, or "Run without a container"): the
  worker inside the app (`Contents/MacOS/myous-worker worker --alias …
  --description … --work ~/.myous-worker/work`, the Go client with the
  review rules built in; nothing to install) as a child process on this
  Mac, started through a login shell so the agent's commands see your
  PATH, output in `~/.myous-worker/worker.log`. Stop sends it SIGTERM:
  it ends the command it runs, answers that request and exits; after ten
  seconds the app kills it. The window warns what this means.
  `MYOUS_WORKER_BIN=<path>` runs another worker program instead (a local
  Go build, for testing); the app's log says so.

The project name is `myous-worker` in every mode, so Stop finds the
containers after a mode switch or an app update; "Remove stale containers"
clears anything older. The mode and checkout path are in
`~/.myous-worker/app.json` with the app's settings (`name`, `dock`,
`notifications` with `notify` per kind, `auto_update`, `show_agents`,
`agent_homes`, and what the owner has already seen); per worker also
`browser` ("mac" or "container"), `browser_app` (the bundle id of the
Mac browser to run), `browser_hidden`, and `description` (what the agent
is told this computer is: the worker's card, `MYOUS_DESCRIPTION` in the
container; absent means the app's default sentence, which names the
worker and says "the myous browser" means its browser).

## What the window shows, screen by screen

- **Set up:** a short list at the top that fills in as the app checks
  the Mac in the background (containers, the runtime with `docker version`
  and `docker compose version`, the agents on this Mac, whether myoushq.com
  answers; "⟳" while a check is out, "couldn't check" with Retry after
  10 s), then the worker's name. Without a runtime: Docker Desktop and
  OrbStack, installed with Homebrew where it is there (`brew install
  --cask docker-desktop` or `orbstack`, the output in the log) or from
  their pages, and the not-recommended no-container option; a runtime
  that isn't running: Open it; no compose: `brew install docker-compose`
  (or the line); an error of its own: said as it said it. The card leaves
  by itself when the runtime answers. If myoushq.com can't be reached, a
  red line says so (Retry); Start stays available.
- **Starting:** four phases with ticks: runtime, download, browser,
  registration. The worker writes `phase` in `worker.json`; the app
  writes the download phase itself. Slow phases turn orange; a failure
  shows the reason.
- **Pair with your agent:** the message to paste (Copy message), the code
  with a validity bar and "New code", a QR code behind a button.
- **Paired:** the verification code; Unpair if the numbers differ.
- **Requests:** newest first, with the agent, what ran or moved, and the
  outcome (ok, exit N, refused with the reason, running…, stopped by
  you). "Stop this command" appears while one runs. New rows are
  bold until the window has been in front; the menu bar shows their count.
  Pause / Resume creates or removes `~/.myous-worker/worker.paused`.
- **Browser:** two places it can run, chosen on the Set up card and under
  Advanced › Browser (a change applies at the next start; each browser
  keeps its own logins). **On this Mac, in …** one entry per installed
  browser the app can run (Google Chrome, Microsoft Edge, Brave or
  Chromium; the first is recommended), else one disabled line and a
  "Get a browser…" pull-down with each one's download page: the app runs
  the chosen browser with the worker's own profile in
  `~/.myous-worker/browser/<bundle id>` (never your everyday profile;
  one profile per browser, so switching browsers means logging in
  again), under a
  seatbelt sandbox profile (`browser.sb`, run with `sandbox-exec`) that
  keeps it to its folder: nothing else under your home folder (not even
  the worker's key next door), no mounted volumes, no programs outside
  its own bundle, no connections to this Mac's localhost; downloads go
  to `~/.myous-worker/downloads`, which the container sees. The window
  is told apart from your own browser by its profile name ("<worker> ·
  myous"), a teal toolbar and a start page that says whose it is. Its
  DevTools port (one the app picks; a dynamic port would make Chrome mark
  every page as automated) is written to `browser.json`, and the
  container's `forward.py` relays `localhost:9222` to it, so agents'
  scripts need no change. It starts hidden (Cmd+H) and keeps working
  there; "Show browser" brings it up (so does Cmd+Tab: hidden is the
  Cmd+H state, not invisible). The setting "Start this worker's
  browser hidden" turns that off for a worker. macOS
  allows no sandbox inside a sandbox, so Chrome's own helper sandbox is
  off (`--no-sandbox`, as in the container, where the container is the
  sandbox); Chrome shows a bar about that flag, and the start page says
so and to close it. If
  the browser can't run (uninstalled, or `sandbox-exec` gone), a red
  line says so. **In the container:** a Chromium inside the container
  started as a plain process (not through Playwright, whose launch marks
  every page as automated, which is what made sites take you for a bot
  in the browser view); "Open browser" asks Docker which port the browser
  view got and opens it connected and sized to the tab. The owner's
  timezone and language go into the container either way.
- **Stopping:** the moment you press Stop, the header, the menu and the
  icon say "Stopping…" (blue) and the card explains; the worker counts
  as stopped as soon as the stop command has finished, without waiting
  for its last status to go stale. If it is still running after 90 s, a
  red line says so.
- A red line at the top asks for the one thing that needs you: the
  runtime is gone, the worker stopped on its own, a release is available.
  When a start failed (or the worker stopped on its own), **Needs
  attention** under it shows the last lines the start wrote, with Show
  log and **Try again** (the same start, no Stop needed).
- **Several workers:** "Add a worker…" (the list on the left, the menu, or
  Advanced) creates `~/.myous-worker-2`, `-3`, … next to the first
  worker's folder, each with its own key, container (compose project
  `myous-worker-2`, …), browser and pairing; its Set up screen asks for
  the name. The list on the left and the menu bar menu show every worker
  (one row each, with its actions in a submenu); the menu bar icon shows
  the worst state. "Remove this worker" is on a worker's Set up and
  Stopped cards, in its submenu in the menu, and in Advanced: a worker
  that was never set up goes at once; one with a key asks first, then is
  stopped and its folder moved to the Trash. The first worker stays (its
  `app.json` holds the app's settings). Stop stops one worker's
  containers only.
- **On this Mac:** when an agent lives on this Mac (a myous client
  directory `~/.myous` or `~/.myous-<name>` with a key, or a folder added
  with "Add an agent's folder…" in the Advanced menu), a list appears on
  the left with the worker and the agents. An agent's card shows what its
  own client reports (`myous status --json`, run in its directory: npub,
  client and version, last used, contacts with relationship and date,
  pairings in progress) and "Pair with code…", which runs the client's
  `accept` as that agent with `--added-by owner`. The Pair screen gains
  "Which agent?": choosing a local agent lets it accept the worker's code
  the same way. The app never reads the agent's files or holds its key.
  Settings: "Show the agents on this Mac" (on by default).

Checking the layout without a worker: `build/myous.app/Contents/MacOS/myous
--snapshot /tmp/window.png` renders the window (a PDF next to it carries
the text) and exits; `MYOUS_FAKE_STATE=setup|checking|hubdown|noruntime|installing|stoppedruntime|nocompose|runtimeerror|
setupnobrowser|starting|failed|pair|paired|running|paused|stopping|stopped|approval|busy|agents|agent|pairlocal|workers|macbrowser`
(`MYOUS_SNAPSHOT_WORKER=<n>` shows the n-th worker)
shows each screen with made-up data, `MYOUS_WORKER_HOME=<dir>` points at
another worker's home, `MYOUS_AGENT_HOMES=<dir>:<dir>` replaces the search
for agents' directories (`MYOUS_SNAPSHOT_AGENT=1` shows the first one's
card), `MYOUS_DOCKER_BIN=/nonexistent` simulates a Mac without Docker, and
`MYOUS_HUB=<url>` is the hub the app checks (and asks for the latest
release): point it at a local server so a test never asks myoushq.com.
`--render-icon DIR` writes the icon PNGs (make-app.sh uses it; the icon
is drawn in code, no image files in the repo: `src/Icon.m` draws the
mark of `worker/icons/mark.svg`). `--browser-test DIR` runs
the "browser on this Mac" launcher for a worker folder on its own: it
prints `browser.json` once the port is open, keeps the browser 20 s and
quits it (`MYOUS_BROWSER_HIDDEN=1` launches it hidden and reports
whether it stayed so).

## Releasing (maintainers)

The release workflow no longer builds this app: it builds
`worker/desktop` (`make-mac.sh --universal --no-dmg --sign …`) and then
runs this folder's `notarize.sh` and `make-dmg.sh` on it. For this app it
ran, in this order:

```sh
worker/mac/make-app.sh --no-config --version 0.7.5 --sign "Developer ID Application: <name> (<team>)"
worker/mac/notarize.sh worker/mac/build/myous.app --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER_ID" --key AuthKey.p8
worker/mac/make-dmg.sh worker/mac/build/myous.app worker/mac/build/myous-v0.7.5.dmg --sign "Developer ID Application: <name> (<team>)"
```

`--sign` uses the hardened runtime and a timestamp (what notarization
requires; the app needs no entitlements), for the worker inside the app
first and then the app. `notarize.sh` zips the app, submits it with an
App Store Connect API key, waits, prints Apple's log on rejection, and
staples the ticket (retrying for up to ten minutes, as the ticket can
take a while to reach Apple's servers). `make-dmg.sh` makes the image
with an Applications shortcut and appends the checksum to `SHA256SUMS`
next to it.

Inputs the workflow needs, as repository secrets:

| Secret | What |
|---|---|
| `MAC_CERT_P12` | the Developer ID Application certificate with its private key, exported as .p12, base64 |
| `MAC_CERT_PASSWORD` | the .p12's password |
| `NOTARY_KEY_ID` | App Store Connect API key ID |
| `NOTARY_ISSUER_ID` | App Store Connect issuer ID |
| `NOTARY_KEY` | the API key's .p8 file, base64 |

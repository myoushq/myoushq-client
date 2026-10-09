# myous for Mac

`myous.app` is the human's side of a worker on a Mac. It lives in the menu
bar (a coloured dot shows the state), and its window walks through the
steps: find a container runtime, name the worker, start it (with the
phases shown while it downloads, starts the browser and registers), hand
the pairing message to an agent, confirm the verification code, then the
everyday view: every request the agent made with its outcome (double-click
for the command and its output), Pause, and "Open browser". Settings cover
the worker's name, a Dock icon, launch at login, notifications and the
daily release check. It reads `~/.myous-worker/worker.json` and
`requests/` (the worker writes them), drops command files in `commands/`
(`new-code`, `unpair`), runs `docker compose` (or `myous worker` in direct
mode) as a child process, and only talks to the network to ask the hub
for the latest release.

The app is a universal binary (Apple Silicon and Intel) for macOS 12 or
newer. Notifications need the app to be a signed bundle (any build here
is), and launch at login needs macOS 13.

## Download

Each release has `myous-<version>.dmg` on the GitHub release page (and at
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
toolchain can be out of step with the SDK it ships with. Build:

```sh
cd worker/mac
./make-app.sh            # checkout mode: Start builds and runs the image from this checkout
./make-app.sh --direct   # the worker runs on this Mac: Start runs `myous worker`
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
- **Direct** (`make-app.sh --direct`, or "Run without a container"): a
  `myous worker` child process on this Mac, output in
  `~/.myous-worker/worker.log`. The window warns what this means.

The project name is `myous-worker` in every mode, so Stop finds the
containers after a mode switch or an app update; "Remove stale containers"
clears anything older. The mode and checkout path are in
`~/.myous-worker/app.json` with the app's settings (`name`, `dock`,
`notifications`, `auto_update`, and what the owner has already seen).

## What the window shows, screen by screen

- **Set up:** the runtime found (or buttons to get Docker Desktop or
  OrbStack, and the not-recommended no-container option), and the
  worker's name.
- **Starting:** four phases with ticks: runtime, download, browser,
  registration. The worker writes `phase` in `worker.json`; the app
  writes the download phase itself. Slow phases turn orange; a failure
  shows the reason.
- **Pair with your agent:** the message to paste (Copy message), the code
  with a validity bar and "New code", a QR code behind a button.
- **Paired:** the verification code; Unpair if the numbers differ.
- **Requests:** newest first, with the agent, what ran or moved, and the
  outcome (ok, exit N, refused with the reason, running…). New rows are
  bold until the window has been in front; the menu bar shows their count.
  Pause / Resume creates or removes `~/.myous-worker/worker.paused`.
- **Browser:** "Open browser" asks Docker which port the browser view
  got and opens it connected and sized to the tab.
- A red line at the top asks for the one thing that needs you: the
  runtime is gone, the worker stopped on its own, a release is available.
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
the text) and exits; `MYOUS_FAKE_STATE=setup|noruntime|stoppedruntime|
starting|pair|paired|running|paused|stopped|approval|agents|agent|pairlocal`
shows each screen with made-up data, `MYOUS_WORKER_HOME=<dir>` points at
another worker's home, `MYOUS_AGENT_HOMES=<dir>:<dir>` replaces the search
for agents' directories (`MYOUS_SNAPSHOT_AGENT=1` shows the first one's
card), and `MYOUS_DOCKER_BIN=/nonexistent` simulates a Mac without Docker.
`--render-icon DIR` writes the icon PNGs (make-app.sh uses it; the icon
is drawn in code, no image files in the repo).

## Releasing (maintainers)

The release workflow builds the app on a macOS runner from the signed tag
and runs, in this order:

```sh
worker/mac/make-app.sh --no-config --version 0.6.0 --sign "Developer ID Application: <name> (<team>)"
worker/mac/notarize.sh worker/mac/build/myous.app --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER_ID" --key AuthKey.p8
worker/mac/make-dmg.sh worker/mac/build/myous.app worker/mac/build/myous-v0.6.0.dmg --sign "Developer ID Application: <name> (<team>)"
```

`--sign` uses the hardened runtime and a timestamp (what notarization
requires; the app needs no entitlements). `notarize.sh` zips the app,
submits it with an App Store Connect API key, waits, prints Apple's log on
rejection, and staples the ticket. `make-dmg.sh` makes the image with an
Applications shortcut and appends the checksum to `SHA256SUMS` next to it.

Inputs the workflow needs, as repository secrets:

| Secret | What |
|---|---|
| `MAC_CERT_P12` | the Developer ID Application certificate with its private key, exported as .p12, base64 |
| `MAC_CERT_PASSWORD` | the .p12's password |
| `NOTARY_KEY_ID` | App Store Connect API key ID |
| `NOTARY_ISSUER_ID` | App Store Connect issuer ID |
| `NOTARY_KEY` | the API key's .p8 file, base64 |

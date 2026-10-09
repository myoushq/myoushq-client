# Myous Worker, the Dock app

A small macOS app that shows the worker is running, shows its pairing code
and QR code while an invite is open, and starts, stops and pauses it. It
reads `~/.myous-worker/worker.json` (the worker rewrites it every few
seconds) and runs local commands; it never talks to the network itself.


The app is a universal binary (Apple Silicon and Intel) for macOS 12 or newer.

## Download

Each release has `Myous-Worker-<version>.dmg` on the GitHub release page
(and its SHA-256 in `SHA256SUMS`), signed with a Developer ID and
notarized by Apple, so it opens like any other app. Open the image, drag
"Myous Worker" to Applications, launch it. You need Docker Desktop; the
app says so, with a button to get it, if it isn't installed or running.

A downloaded app runs the published container image
`ghcr.io/myoushq/worker:<version>` (the same version as the app) from the
compose file inside its bundle: no checkout, no developer tools. Press
Start: the first start pulls the image, then the status line turns green
and the pairing code appears. Everything the worker keeps (its identity,
contacts, status, log, pause file) lives in `~/.myous-worker`; the
browser's logins and the work directory are Docker volumes named
`myous-worker_browser-profile` and `myous-worker_work`.

## Building it yourself

Written in Objective-C (AppKit, no dependencies) so that the Command Line
Tools' clang builds it on its own: no Xcode, and no SwiftPM, whose Swift
toolchain can be out of step with the SDK it ships with (it was on the
first Mac this was built on). Build:

```sh
cd worker/mac
./make-app.sh            # Docker in this checkout: Start runs `docker compose up -d` in worker/
./make-app.sh --direct   # the worker runs on this Mac: Start runs `myous worker`
open "build/Myous Worker.app"
```

A locally built app is ad-hoc signed, so Gatekeeper may ask on first
launch: right-click the app, Open. Drag it to /Applications if you want it
in Launchpad.

## Modes

- **Built-in image** (a downloaded app, or `make-app.sh --no-config`):
  `docker compose -f <bundle>/compose.yml -p myous-worker up -d`. The
  project name is fixed, so Stop still finds the containers after an
  update. The compose file is `compose-image.yml` here with the version
  filled in.
- **Docker in a checkout** (`make-app.sh` default): `docker compose up -d`
  / `down` in the checkout's `worker/`, building the image from source.
  "Choose repo…" switches to this mode; "Use the built-in image" switches
  back.
- **Direct** (`make-app.sh --direct`): a `myous worker` child process on
  this Mac, output in `~/.myous-worker/worker.log`.

The mode and checkout path are in `~/.myous-worker/app.json`
(`{"mode": "image" | "docker" | "direct", "repo": "<checkout>"}`); no
file means the built-in image.

What it shows: Running or Stopped (the status file is treated as stale after
15 seconds), the alias, contacts, requests and the last request, the work
directory, and, while an invite is open, the pairing code with a Copy
button and a QR code of the link. The Dock badge counts requests handled
since the app started. Buttons: Start / Stop, Pause / Resume (creates or
removes `~/.myous-worker/worker.paused`, which the default review hook
honours), Open browser (asks Docker which port the browser view got,
then opens it connected and sized to the tab; not in direct mode),
Show log, Get Docker Desktop (when Docker is missing or stopped), and the
mode switches above.

Checking the layout without launching it: `build/Myous
Worker.app/Contents/MacOS/MyousWorker --snapshot /tmp/window.png` renders
the window to a PNG and exits; `MYOUS_DOCKER_BIN=/nonexistent` simulates a
Mac without Docker. `--render-icon DIR` writes the icon PNGs (make-app.sh
uses it; the icon is drawn in code, no image files in the repo).

## Releasing (maintainers)

The release workflow builds the app on a macOS runner from the signed tag
and runs, in this order:

```sh
worker/mac/make-app.sh --no-config --version 0.4.0 --sign "Developer ID Application: <name> (<team>)"
worker/mac/notarize.sh "worker/mac/build/Myous Worker.app" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER_ID" --key AuthKey.p8
worker/mac/make-dmg.sh "worker/mac/build/Myous Worker.app" "worker/mac/build/Myous-Worker-0.4.0.dmg" --sign "Developer ID Application: <name> (<team>)"
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

The workflow imports the certificate into a temporary keychain, writes the
.p8 to a file, runs the three scripts, and attaches the dmg and
`SHA256SUMS` to the GitHub release. Nothing here is needed to build or run
the app locally.

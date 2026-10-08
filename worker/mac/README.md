# Myous Worker, the Dock app

A small macOS app that shows the worker is running, shows its pairing code
and QR code while an invite is open, and starts, stops and pauses it. It
reads `~/.myous-worker/worker.json` (the worker rewrites it every few
seconds) and runs local commands; it never talks to the network itself.

Written in Objective-C (AppKit, no dependencies) so that the Command Line
Tools' clang builds it on its own: no Xcode, and no SwiftPM, whose Swift
toolchain can be out of step with the SDK it ships with (it was on the
first Mac this was built on). Build:

```sh
cd worker/mac
./make-app.sh            # Docker mode: Start runs `docker compose up -d` in worker/
./make-app.sh --direct   # the worker runs on this Mac: Start runs `myous worker`
open "build/Myous Worker.app"
```

The app is ad-hoc signed, so Gatekeeper may ask on first launch: right-click
the app, Open. Drag it to /Applications if you want it in Launchpad.

What it shows: Running or Stopped (the status file is treated as stale after
15 seconds), the alias, contacts, requests and the last request, the work
directory, and, while an invite is open, the pairing code with a Copy
button and a QR code of the link. The Dock badge counts requests handled
since the app started.

Buttons: Start / Stop (Docker: `docker compose up -d` / `down` in this
checkout's `worker/`; direct: a `myous worker` child process, output in
`~/.myous-worker/worker.log`), Pause / Resume (creates or removes
`~/.myous-worker/worker.paused`, which the default review hook honours),
Open browser view (`http://localhost:6080`, Docker mode only), Show log,
and Choose repo… if the checkout moved.

Checking the layout without launching it: `build/Myous
Worker.app/Contents/MacOS/MyousWorker --snapshot /tmp/window.png` renders
the window to a PNG and exits. `--render-icon DIR` writes the icon PNGs
(make-app.sh uses it; the icon is drawn in code, no image files in the
repo).

How it finds the repo: `make-app.sh` writes `~/.myous-worker/app.json`
with `{"repo": "<this checkout>", "mode": "docker" | "direct"}`. Edit it,
or use Choose repo…, if you move the checkout.

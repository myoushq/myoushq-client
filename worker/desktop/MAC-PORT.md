# myous for Mac on Tauri: draft (phase 1)

A draft on the branch `wip/mac-tauri`: this app (`worker/desktop`) built
and run on macOS, so the owner can judge whether it can replace
`worker/mac` (Objective-C). Phase 1 of `TAURI-PORT-SPIKE.md` in the
private repo. Nothing here changes the app on Linux or Windows: the Mac
parts are behind `cfg(target_os = "macos")` or the window's
`info.os === "macos"`. `worker/mac` is untouched.

## What works on a Mac

- **A menu bar app.** `src-tauri/Info.plist` (merged by Tauri) sets
  `LSUIElement`; at start the app reads `dock` from the first worker's
  `app.json`, where myous for Mac keeps it, and shows a Dock icon only
  if it is on. macOS 13 or newer, as myous for Mac since 2026-10-12
  (the Go worker inside needs 13).
- **The menu bar icon** is the mark on a square in the state's colour,
  drawn at 36 pixels for its 18 points (two per point on Retina) with
  `Icon.m`'s proportions, the white dot in the corner while a request
  runs. The count of new requests stands next to it and on the Dock
  icon while the window is away, as in myous for Mac.
- **Settings › "Show an icon in the Dock as well as the menu bar"**,
  applied at once. A click on the Dock icon shows the window.
- **Open at login** through the autostart plugin: a LaunchAgent,
  `~/Library/LaunchAgents/myous.plist`, pointing at the program inside
  the app. Not SMAppService (see below).
- **Notifications** without buttons; a question brings up the window,
  which has Allow and Refuse (as on Linux and Windows).
- **Workers in a container** (Docker Desktop, OrbStack, Rancher
  Desktop), found from the Finder's short PATH, with their browser on
  the Mac or in the container (below). The owner's language comes from
  the Mac's preferred languages when the app has no locale variables
  (an app opened from the Finder has none).
- **The worker's browser on the Mac**, for a worker in a container, as
  myous for Mac runs it (`Browser.m`, ported into `browser.rs`'s `mac`):
  Set up asks "Where should its browser run?" ("On this Mac, in Google
  Chrome (recommended)", one line per installed browser, then "In the
  container"; with none, a disabled line that says why and "Get a
  browser…"), and Settings has the same choice with "Start this worker's
  browser hidden" (a running worker offers a restart, as myous for Mac
  does). `app.json` keeps `browser: "mac"`, `browser_app` and
  `browser_hidden` with myous for Mac's meanings, so an upgrade keeps the
  owner's choice. The container gets `MYOUS_BROWSER=host`; the app starts
  the browser (found through Launch Services by bundle id, else in
  `/Applications` or `~/Applications`) with its profile in
  `<worker folder>/browser/<bundle id>` and downloads in
  `<worker folder>/downloads`, on a port it chooses, and writes myous for
  Mac's `browser.json` (`host: host.docker.internal`, `port`, `pid`,
  `app`, `at`; no `kind`), which `forward.py` relays the container's
  localhost:9222 to; the worker's help in the container is the same as
  with the container's browser. The browser lives with the worker:
  started while it starts and runs (one the owner quits comes back),
  ended when it stops or the app quits, a browser left from a run before
  taken over. Hidden after launch when `browser_hidden` says so (again
  every half second for six seconds, as myous for Mac does); Show browser
  unhides and activates it through AppKit. The Browser card, the start
  rows and the menu item say what myous for Mac says; when it can't run,
  the red line says why (no browser: "Get a browser…"; no sandbox: the
  log).
- **The sandbox**: that browser, and the one of a worker without a
  container, runs under `/usr/bin/sandbox-exec -f <worker
  folder>/browser.sb` with myous for Mac's seatbelt profile (word for
  word; a test compares it, the probe and the start page with
  `Browser.m`), `--no-sandbox` inside it and myous for Mac's start page
  that explains Chrome's bar about it. The probe (`sandbox-exec` with a
  small profile) decides: without a sandbox a container worker gets the
  container's browser, with myous for Mac's message; a worker without a
  container runs its browser unconfined, as on Linux and Windows, and the
  log says so. `cargo test -- --ignored real_browser` runs it for real.
- **Workers without a container**: the Go worker inside the app, and the
  worker's browser on the Mac (Chrome, Edge, Brave or Chromium, as
  above, under the sandbox), with a profile in
  `<worker folder>/browser/<bundle id>`, the folder myous for Mac uses,
  so its logins carry over. Python is never `/usr/bin/python3` (the
  Command Line Tools' 3.9, too old for Playwright, and on a Mac without
  those tools running it puts up the dialog to install them); "Get
  Python" opens python.org's macOS page.
- **The Mac's look** (html.os-macos, set from the system at start; the
  Linux and Windows window is pixel for pixel what it was): 13 point
  text with 11 point captions and table; the window colour (#ececec,
  #323232 in dark mode) with grey group boxes, the title above each box;
  WebKit's own push buttons and pop-up menus, the primary button, links
  and the progress bar in the accent colour; the system's colours
  (`-apple-system-label`, `-apple-system-control-accent`, the alternating
  row colours, ...), so dark mode, the accent colour and Increase
  Contrast follow System Settings; no resize grips, and the pairing
  message as grey read-only text in a sunken box, as tall as it is; the
  requests in AppKit's striped table, ten rows high, the selected row in
  the accent colour. WKWebView has no name for the window colour, so it
  is written out per mode; the previews in Chromium use the same colours
  written out.
- **A window that fits its content**, as myous for Mac's: 560 points
  wide (and the list of workers when there are several), as high as the
  screen in front needs, its top edge staying put, not resizable; a
  dialog (Settings, a log) gets the room it needs. The page measures
  itself and asks Rust (`fit_window`).
- **The app menu** holds what the ⋯ menu holds on Linux and Windows
  (Settings… on ⌘,, Check for updates…, the browser, the logs and the
  worker folder, adding workers and agents' folders, the modes), with
  About, Hide and Quit, and Edit and Window menus; the window has no ⋯,
  and Running has **Open log** (the app log) under the requests.
- **The default worker name is the Computer Name** ("Max's MacBook
  Air", `scutil --get ComputerName`, as myous for Mac takes it), the
  host name only if that can't be read. The previews for macOS say
  "Max's Mac".
- Everything else is the shared app: Set up and the first-launch
  checks, runtime install with Homebrew, pairing, requests, review,
  several workers, the agents on the Mac, the update check against
  `/download/mac`.

## Phase 2 (not built)

- **Allow and Refuse on the notification.** The notification plugin
  (2.6.0) can show actions on macOS through `notify-rust`
  (NSUserNotification: two actions become an "Options" menu, and a
  shown notification can't be withdrawn), or the app talks to
  UserNotifications itself (`objc2-user-notifications`), which is what
  myous for Mac does.
- **A real container worker with its browser on the Mac** has not been
  run end to end (Docker on the test Mac): the app side is tested (the
  browser, `browser.json`, the port, hide and show, the sandbox), and
  `forward.py` is myous for Mac's, unchanged.
- **SMAppService** for open at login. `auto-launch` 0.6, already in the
  dependency tree under the autostart plugin, has an SMAppService mode
  the plugin doesn't offer; using it directly would be a few lines.
  An upgrade from myous for Mac would also need to remove the old
  login item, or the owner sees two.
- **Signing and notarizing in CI** (a `release.yml` job; see below).
- The Option-held Advanced menu (it is always there), separate Settings
  and log windows, `--snapshot` and `--browser-test`.

## Build and try it

```sh
cd worker/desktop
./make-mac.sh                 # build/myous.app (ad-hoc signed) and build/myous-<version>.dmg
./make-mac.sh --universal     # Apple Silicon and Intel (rustup target add aarch64-apple-darwin x86_64-apple-darwin)
open build/myous.app
```

`make-mac.sh` runs `tauri build --bundles app --no-sign`, signs the
worker and then the app as `worker/mac/make-app.sh` does (`--sign
"Developer ID Application: …"` for the hardened runtime and a
timestamp), and then uses `worker/mac/notarize.sh` (with `--key-id`,
`--issuer`, `--key`) and `worker/mac/make-dmg.sh` unchanged. Inside the
app the program is `Contents/MacOS/myous-desktop` (not on PATH, so it
never stands in for the client's `myous`) with `myous-worker` next to it.

To look without touching a real worker: `MYOUS_WORKER_HOME=/tmp/w
MYOUS_FAKE_STATE=pair build/myous.app/Contents/MacOS/myous-desktop`.
The previews: `python3 screens.py OUT --os macos` (each at the size the
window takes on a Mac; `setup` has the browser question, `macbrowser` is
a container worker with its browser on the Mac).

Screenshots from the draft's run (2026-10-14, in the session's
scratchpad, `mac-tauri-*.png`): the window's states as the previews draw
them for macOS (`mac-tauri-window-*.png`), the menu bar icon in each
state (`mac-tauri-tray-icons-4x.png`) and on a menu-bar-like strip
(`mac-tauri-menubar-mock.png`). `screencapture` of the running app
didn't work there: the shell had no Screen Recording permission, so
the capture left out other apps' windows and menu bar items. The
running app's window (640 by 701 points) and menu bar item (34 points
wide, 50 with a count) were checked through the window list instead.
Playwright's WebKit hung at launch on that Intel Mac, so
`screens.py --engine webkit` is untested there.

A CI job would need: a macOS runner; Node 22, Rust with both Mac
targets, Go; `./make-mac.sh --universal --sign "$IDENTITY" --key-id …
--issuer … --key …` with the certificate imported into a temporary
keychain as the Mac job does now; then the dmg uploaded as
`myous-<version>.dmg`.

## Differences a Mac user would notice

- The window is a web page in the system's web view, dressed as the
  Mac app (its colours, group boxes, push buttons and table; see above),
  with the shared layout: the runtime rows, the approval banner, the
  ok / refused column. Settings and the logs open inside it, not in
  windows of their own.
- A question's notification has no buttons; it brings up the window.
- The browser choice is in Settings (and on Set up), not under the
  Option-held Advanced › Browser; Brave is named "Brave", not its
  bundle's "Brave Browser". A profile from before myous for Mac kept one
  per browser (`browser/Default`) isn't moved into
  `browser/<bundle id>`; myous for Mac has moved it already on any Mac
  that ran a recent version.
- Open at login is listed under "Allow in the Background" in System
  Settings (a LaunchAgent), not as a login item, and it points at
  where the app was when it was turned on.
- New: the first-launch checks, the runtime install cards, the app
  menu's worker items (myous for Mac has them under the menu bar icon's
  Advanced).

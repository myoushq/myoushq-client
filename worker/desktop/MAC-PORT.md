# myous for Mac on Tauri

This app (`worker/desktop`) is myous for Mac from the release after
v0.7.5, replacing `worker/mac` (Objective-C, the myous for Mac that
v0.7.5 and earlier shipped), which is kept in the repository for now and
no longer built or released. Where this file compares the two, "myous
for Mac" is that Objective-C app. This file says what the Mac parts are,
how the app is built and released for the Mac, and what a Mac user
notices. Nothing here changes the app on Linux or Windows: the Mac parts
are behind `cfg(target_os = "macos")` or the window's
`info.os === "macos"`.

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
- **Open at login** as a login item through SMAppService
  (`src-tauri/src/loginitem.rs`), as myous for Mac: the `auto-launch`
  crate under the autostart plugin, in its SMAppService mode, which the
  plugin doesn't offer. Settings' checkbox reads and sets it; it shows
  under System Settings › General › Login Items › Open at Login. If the
  owner turned myous off there, ticking the box can't turn it back on
  (macOS wants the owner to do it there), and the log says so. On the
  first start after an upgrade from this draft's earlier builds, the
  autostart plugin's LaunchAgent (`~/Library/LaunchAgents/myous.plist`)
  is removed and, since it was on, the app registers instead, so there is
  never a second entry. The bundle identifier is myous for Mac's
  (`com.myoushq.desktop`), so a login item the Mac app registered is this
  app's too. A program run from `target/` (not an app) keeps the plugin's
  LaunchAgent, as SMAppService needs the bundle.
- **Notifications through UserNotifications**
  (`src-tauri/src/usernotify.rs`), as myous for Mac's: a question comes
  with **Allow** and **Refuse** (the category "approval", the
  notification `ask-<request id>`), and a press answers it through the
  same Rust path as the window's buttons (`answer_approval`, in the worker
  whose `approvals/` holds the request, as myous for Mac looks it up); the
  window logs "allow (from the notification): ..." and drops the
  question. The window doesn't come up for a question then. A click on
  any notification brings up the window. The notification goes when the
  question is answered in the window (`answer_approval`), or when it
  stops waiting (answered elsewhere, or timed out: `read_workers` sees it
  gone). Permission is asked once, at the first start; if the owner
  refuses (or later turns notifications off; the app rechecks at each
  notification, so the change counts from the next one), there is no
  notification and the app brings up the window for a question, as
  before; the app log says why ("notifications: not allowed (...)").
  The plugin's plain notification can't stand in there: usernoted
  refuses a process that mixes the old notifications (the plugin's,
  NSUserNotification) with UserNotifications, and the permission is
  the same one anyway. Every notification goes through
  UserNotifications, so a Mac has one kind and one click behaviour; the
  plugin stays for Linux and Windows. UserNotifications aborts a process
  that isn't an app bundle, so a program run from `target/` and `cargo
  test` never touch it (`in_app_bundle`: a bundle identifier and a path
  ending in `.app`) and keep the plugin's plain notifications.
- **Built and tried (2026-10-14), not yet pressed.** `cargo build`,
  `cargo test`, clippy and rustfmt pass on a Mac, `npm test` and
  `clicks.py` pass, and the Linux and Windows previews are pixel for
  pixel those of `wip/mac-tauri`. The app (`make-mac.sh`, ad hoc) was
  run with a worker folder holding `app.json` (a name), a fresh
  `worker.json` (`{"phase":"running","contacts":1}`, so the window starts
  hidden) and then `approvals/r5.json`
  (`{"id":"r5","op":"exec","cmd":"ls","asked_at":<now>,"wait":120}`).
  It set up the centre and the category and asked for permission; on
  that Mac macOS already held a decision for `com.myoushq.desktop` (the
  identifier myous for Mac's builds share), so no dialog came and the
  answer was "Notifications are not allowed for this application"
  (code 1). The question then brought up the window, as it should. Still
  to see, with notifications allowed for myous in System Settings ›
  Notifications: the permission dialog on a Mac that hasn't decided, the
  notification with Allow and Refuse, a press answering it, a click
  bringing up the window, and the notification going when the window
  answers. Open at login: no `~/Library/LaunchAgents/myous.plist` was
  made; Settings' checkbox (SMAppService's status) wasn't opened, as
  that needs a click in the window. The fake states
  (`MYOUS_FAKE_STATE`) never notify.
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
  Not yet run end to end with a real container worker (no Docker on the
  test Mac): the app side is tested (the browser, `browser.json`, the
  port, hide and show, the sandbox), and `forward.py` is myous for Mac's,
  unchanged.
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
  About, Hide and Quit, and Edit and Window menus. It shows in the menu
  bar while the Dock setting is on. With the setting off (the default)
  the app is an accessory app, as myous for Mac was, and macOS may show
  no menus for it (not tried). The ⋯ items are also in the menu bar
  icon's menu: Settings…, Check for updates…, the browser item, and the
  rest under Advanced. The window has no ⋯, and Running has **Open
  log** (the app log) under the requests.
- **The default worker name is the Computer Name** ("Max's MacBook
  Air", `scutil --get ComputerName`, as myous for Mac takes it), the
  host name only if that can't be read. The previews for macOS say
  "Max's Mac".
- Everything else is the shared app: Set up and the first-launch
  checks, runtime install with Homebrew, pairing, requests, review,
  several workers, the agents on the Mac, the update check (the hub's
  `latest_release` in `/config.json`; its Download button opens
  `/download/mac`).

## Not ported from myous for Mac

- **The Option-held Advanced menu**: not planned. The menu bar icon's
  menu always has Advanced, and its items are in the app menu too while
  the Dock setting is on (the ⋯ menu on Linux and Windows), so there is
  nothing to hold Option for.
- **Separate Settings and log windows**: not planned. They open inside
  the window, as on Linux and Windows; chosen from the menu bar icon's
  menu while the window is closed, they bring the window up first.
- **`--snapshot` and `--browser-test`**: later, if at all. `screens.py`
  (`--os macos`) draws every screen and `cargo test -- --ignored
  real_browser` runs the browser under the sandbox, which is what the
  two did for myous for Mac.

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

`--universal` builds the worker for each architecture and joins them
with `lipo` into `binaries/myous-worker-universal-apple-darwin`: `tauri
build --target universal-apple-darwin` compiles each architecture on its
own, and each compile's build script copies the worker for its own
target (`-aarch64-apple-darwin`, `-x86_64-apple-darwin`), while the
bundle takes the universal one. It stops with the `rustup target add`
line when a target is missing, and fails unless both programs in the
app hold arm64 and x86_64.

## Release

The release builds myous for Mac from this app (`release.yml`, job
`mac-app`, "macOS app (signed and notarized when the secrets exist)",
macOS 15 runner, 90 minutes): Node 22, Rust stable with
`aarch64-apple-darwin` and `x86_64-apple-darwin`, Go from `go/go.mod`;
the version in `package.json` checked against the tag; `npm ci && npm
test && npm run build:ui` and `cargo test` on the runner; the Developer
ID certificate imported into a throwaway keychain as before; then

```sh
./make-mac.sh --universal --no-dmg --sign "$identity"   # ad hoc, without --sign, when the secrets are missing
worker/mac/notarize.sh worker/desktop/build/myous.app --key-id … --issuer … --key …
worker/mac/make-dmg.sh worker/desktop/build/myous.app out/myous-$TAG.dmg --sign "$identity"
```

and `out/` (the dmg and `SHA256SUMS`) uploaded as the artifact
`mac-app`, which the `github-release` job attaches as before. The asset
keeps its name, `myous-<tag>.dmg`, which is what the hub's
`/download/mac` serves. `worker/mac` is no longer built by the release
or by `ci.yml`, whose macOS job now runs `npm test`, `cargo test` and an
ad-hoc `make-mac.sh` (the runner's architecture only, with its dmg).

Not checked before a tag: the universal build itself (the draft's Mac
lacked the disk for it), signing with the Developer ID and
notarization of this app. The first release run is the test; if
notarization rejects the app, Apple's log is in the job's output.

## Differences a Mac user would notice

- The window is a web page in the system's web view, dressed as the
  Mac app (its colours, group boxes, push buttons and table; see above),
  with the shared layout: the runtime rows, the approval banner, the
  ok / refused column. Settings and the logs open inside it, not in
  windows of their own.
- The browser choice is in Settings (and on Set up), not under the
  Option-held Advanced › Browser; Brave is named "Brave", not its
  bundle's "Brave Browser". A profile from before myous for Mac kept one
  per browser (`browser/Default`) isn't moved into
  `browser/<bundle id>`; myous for Mac has moved it already on any Mac
  that ran a recent version.
- New: one Install per runtime with a Website link, and the app menu's
  worker items while the Dock setting is on (both apps also have them
  under the menu bar icon's Advanced, which myous for Mac showed only
  with Option held, in a checkout or in direct mode).
- Remove stale containers… asks which containers go (just the stopped
  ones, or all; this app's own worker only when it runs an older
  version, and only when ticked). The Objective-C app removed every
  worker container, the running worker's too.

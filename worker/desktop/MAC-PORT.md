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
  Desktop), found from the Finder's short PATH, with the container's
  own browser and its browser view (`MYOUS_BROWSER: container`). The
  owner's language comes from the Mac's preferred languages when the
  app has no locale variables (an app opened from the Finder has none).
- **Workers without a container**: the Go worker inside the app, and the
  worker's browser on the Mac: Chrome, Edge, Brave or Chromium from
  `/Applications` or `~/Applications`, with a profile in
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
- **The browser on the Mac for a worker in a container**
  (`MYOUS_BROWSER=host`, the forwarder), with Set up's "Where should its
  browser run?" (myous for Mac recommends "On this Mac, in Google
  Chrome"): the one feature a Mac user would see missing, needed before
  this app replaces myous for Mac; and **the sandbox**
  (`sandbox-exec` with myous for Mac's seatbelt profile). Today a
  container worker uses the container's browser, and a worker without a
  container runs its browser unconfined, as on Linux and Windows,
  where myous for Mac runs it under the seatbelt. Hide and show of
  that browser too.
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
window takes on a Mac).

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
- A worker in a container uses the container's browser (through the
  browser view), not Chrome on the Mac.
- Open at login is listed under "Allow in the Background" in System
  Settings (a LaunchAgent), not as a login item, and it points at
  where the app was when it was turned on.
- New: the first-launch checks, the runtime install cards, the app
  menu's worker items (myous for Mac has them under the menu bar icon's
  Advanced).

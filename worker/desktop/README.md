# myous for Mac, Linux and Windows

`myous` is the human's side of a worker on a Mac, Linux or Windows
computer: a menu bar or tray icon whose colour shows the state, and a
window that walks through the steps. Find a container runtime, name the worker, start it (with the
phases shown while it downloads, starts the browser and registers), hand
the pairing message to an agent, confirm the verification code, then the
everyday view: every request the agent made with its outcome (click a row
for the command and its output), Allow and Refuse when the review level
asks, Pause, Stop this command, and "Open browser". One codebase for the
three systems: the same screens and words, the same worker container, the
same files in the worker folder, so a folder moves between them
unchanged. On a Mac it replaces the earlier Objective-C app
(`worker/mac`), which reads and writes the same folder; "On a Mac" below
says what differs.

## Download

Each release has, on the GitHub release page and at the links below (their
SHA-256 sums are in `SHA256SUMS`):

- **Mac:** `myous-v<version>.dmg` (https://myoushq.com/download/mac),
  one app for Apple Silicon and Intel, macOS 13 or newer, signed with a
  Developer ID and notarized by Apple. Open the image, drag myous to
  Applications, open it. For containers you need Docker Desktop,
  OrbStack or Rancher Desktop (the app installs Docker Desktop or
  OrbStack with Homebrew where Homebrew is there).
- **Windows:** `myous-v<version>-windows-x64-setup.exe`
  (https://myoushq.com/download/windows). An installer for your user
  only, no administrator rights; it fetches Microsoft's WebView2 if the PC
  lacks it (Windows 11 has it). It is not code-signed yet, so SmartScreen
  asks once: More info, then Run anyway. For containers you need Docker
  Desktop or Rancher Desktop, running; both need WSL 2. Without it, see
  "Without containers" below.
- **Linux:** `myous-v<version>-linux-x86_64.AppImage`
  (https://myoushq.com/download/linux): make it executable and run it.
  Or `myous-v<version>-linux-amd64.deb` for Debian and Ubuntu. You need
  Docker Engine (add yourself to the `docker` group), Podman, or Docker
  Desktop. On GNOME the tray icon needs the AppIndicator extension
  (Ubuntu has it on).

The app runs the published container image `ghcr.io/myoushq/worker:<version>`
(the same version as the app) from the compose file built into it
(`compose.yml` here, the same services as `worker/mac/compose-image.yml`;
a test keeps them equal). The first start pulls about 2 GB. Everything the
worker keeps lives in `~/.myous-worker` on a Mac and on Linux, and
`%LOCALAPPDATA%\myous\worker` on Windows; the browser's logins and the work
directory are container volumes. The app writes its own log to `app.log`
there.

With a container the worker's browser runs in the container (shown
through the browser view in your own browser), or on a Mac on the Mac if
you choose (see "On a Mac"); without one it runs on the computer (below).
On Linux and Windows notifications have no buttons: a question brings up
the window, which has Allow and Refuse.

## The first launch

The window draws at once and checks the computer in parallel: whether
containers can run here (Windows: WSL 2 and virtualization), the runtime
(`docker version` and `docker compose version`, 5 s each), the agents on the
computer, and whether it reaches myoushq.com (one GET of `/config.json`,
5 s). The answers fill a short list at the top of the Set up card (✓, ✗,
"⟳" while a check is out; after 10 s without an answer, "couldn't check"
with Retry, never a tick or a cross it didn't earn; a row that moves on,
say Python found, flashes green and says what it found and what comes
next for 8 s), and the rest of the card is the one thing still needed:

- **Set up** (the form) once the runtime answers, or after 10 s if it
  doesn't.
- **Get a container runtime** when there is none: Docker Engine, Podman and
  Docker Desktop on Linux, Docker Desktop and Rancher Desktop on Windows.
  Where the tool for it is there the app installs it itself (**Install**:
  winget on Windows; on Linux apt, dnf or pacman through `pkexec`, which
  asks for the password, and for Docker Engine also starts the service and
  adds you to the `docker` group), with the output in the app log;
  otherwise the button opens its download page. Until the app knows
  which ones this system can have (it asks the package manager, which can
  take a while on a cold start), the card says "Looking for ways to
  install one…". The card leaves by itself when the runtime answers.
  Folded underneath: "Run without a container (not recommended)".
- **Start the container runtime** when it is installed but not running:
  "Open Docker Desktop" (or Rancher Desktop); for Docker Engine on Linux
  "Start Docker Engine" (`pkexec systemctl enable --now docker`) where
  pkexec and systemd are there, else the line to type.
- **Allow yourself to use it** when the runtime says permission denied:
  the `docker` group line (Windows: `docker-users`), then log out and in.
- **Install compose** when `docker compose` is missing (Debian's and
  Ubuntu's `docker.io` has none): the distribution's package
  (`docker-compose-v2`, `docker-compose-plugin` from Docker's repository,
  `podman-compose` for Podman), installed through pkexec or as the line to
  type.
- **Container runtime** with the runtime's own error, when it answers with
  one.
- **Turn on WSL** and **Containers can't run here** (Windows), below.

While a Set up card is up the runtime is checked every 2 s and the hub
every 15 s. When the hub can't be reached a red line says so on whatever
card is up, with Retry; Start stays available (the worker registers once
it can reach the hub). When a start fails, **Needs attention** under the
red line shows the last lines it wrote, with Show log and **Try again**
(the same start, no Stop needed).

An app opened from a desktop launcher gets a short PATH, so the app
looks for programs (the runtime and its compose, Python, the browser,
the package tools) on the PATH it inherited and then in the folders
where they install: on Linux `/usr/local/bin`, `~/.local/bin`,
`/snap/bin`, Flatpak's and Rancher Desktop's; on Windows Docker
Desktop's, Podman's, Rancher Desktop's, the per-user Pythons and
winget's. The commands it starts get the same PATH, so `docker compose`
finds its credential helper (`search_path` in `engine.rs`).

## Without containers

On Windows the runtimes need WSL 2, and WSL 2 needs virtualization. When
there is no runtime, the app checks (`containers.rs`): WSL not turned on
on a PC that can have it offers **Turn on WSL…** (`wsl --install
--no-distribution`, as administrator, then a restart); a virtual machine
that doesn't pass virtualization on (an AWS WorkSpace, most cloud
desktops), or virtualization turned off in the firmware, says so. Either
way, and on Linux as an option, the owner can go on without containers:
the app remembers it (`"containers": false` in the first worker's
`app.json`), goes straight to Set up, every worker then runs directly on
the computer, as the Objective-C Mac app's direct mode does, and the ⋯ menu offers
the way back. Without a container, commands run as the owner
(PowerShell on Windows, `/bin/sh` elsewhere), and the review starts at
"Ask me before anything that changes things". The agents on the
computer are found and paired the same way.

**The worker's browser without a container** (`browser.rs`) runs on the
computer. The app looks for Edge then Chrome on Windows (where their
installers put them, else the registry's App Paths), `google-chrome`,
`chromium` or `microsoft-edge` on the search path (above) on Linux (not
a snap's Chromium, which can't use a profile in a hidden folder), and
for Playwright in an environment of its own
(`%LOCALAPPDATA%\myous\browser-python`, or
`~/.local/share/myous/browser-python`). The owner provides Python 3.10
or newer (`py -3`, `python`, `python3` on Windows; `python3`, `python`,
`python3.1x` elsewhere); the Set up card says what is missing, one thing
at a time: a browser ("Get Edge", "Get Chrome"), Python ("Get Python",
python.org on Windows; the distribution's package line on Linux, with
`python3-venv` on Debian and Ubuntu), or Playwright (**Install
Playwright**: `python -m venv` with that Python, then `pip install
playwright==<the container's version>`, the output in the app log; no
`playwright install`, as no browser is downloaded). When a worker
starts, the app starts the browser first: `--user-data-dir=<folder>/browser/<msedge|chrome|chromium>`
(its own profile, named "<worker> · myous", downloads into
`work/downloads`), `--remote-debugging-port=<a free port>` and a start
page (`browser/start.html`) that says whose window it is. It is a plain
process, never launched through Playwright, so pages aren't marked as
automated. The port goes into `browser.json` once it answers (the keys
the Objective-C Mac app writes, plus `python` and `kind: "host"`), which the worker
reads at each `help`: a line with the address for Playwright's
`connect_over_cdp`, and on the shell line the Python that has Playwright
(as PowerShell takes it on Windows), its version, and whether the plain
`python` lacks Playwright. A worker started without its browser (before
Playwright was installed, say) gets one from "Open browser" on the
Browser card, no restart: its next `help` names it. The browser stops
with the worker and when the app quits (asked to close first, ended
after 8 s); "Show browser" (the Browser card, the tray, the ⋯ menu)
brings its page to the front, or starts it again on the same port if
the owner closed it. "Continue
without a browser" on the Set up card, or the checkbox in Settings,
turns it off for that worker (`"browser": "none"` in its `app.json`;
`"host"` turns it back on; a change while the worker runs takes effect
at its next start). While it is on, the worker's default description
says, as the container worker's does, that its browser is where the
owner logs into sites ("the myous browser" means that one).

The worker is the Go client's `myous worker`, which the app carries inside
itself as `myous-worker` (`myous-worker.exe`; never `myous`, which on
Windows is the app): a Tauri bundled program (`externalBin`), so it lands
next to the app's own program in every package (`/usr/bin` in the deb,
`usr/bin` inside the AppImage, the install folder on Windows) and the app
runs it from there by its full path. It is always the app's own version,
and nothing needs installing: no Python, no pip. The app starts it as
`myous-worker worker --alias … --description … --work <folder>/work`
with `MYOUS_HOME` set to the worker folder; the review rules are built
into it (the level in `review.json`, `worker.paused`, the questions in
`approvals/`), the same as `worker/review.py`'s. The worker's own output
is in `worker.log` (Show worker log, in the ⋯ menu). Stop writes a
`commands/quit` file in the worker folder (and sends SIGTERM outside
Windows): the worker ends the command it runs with everything it
started, answers that request "stopped by the owner" and exits; after
ten seconds the app kills it. When the app quits it waits for its
workers the same way. A worker the app didn't start (found through the
pid in `worker.json`) is stopped the same way only while that pid is
still the worker: a myous worker program (`myous-worker`, `myous` or
Python) that began no later than `worker.json`'s `started`, as `ps` (on
Windows PowerShell's `Get-Process`) tells; otherwise, or if that can't
be told, it is left alone and the log says why.

## How it is built

[Tauri 2](https://tauri.app): a small Rust program that owns the tray, the
worker folders and the container runtime, and a window drawn by the
system's web view (WKWebView on a Mac, WebView2 on Windows, WebKitGTK on
Linux). The window is plain HTML, CSS and TypeScript with no framework. An earlier version
drew with OpenGL (Go and Fyne) and showed no window at all on PCs
without a graphics driver, such as cloud desktops and many virtual
machines; a web view has no such need.

| Where | What |
|---|---|
| `ui/src/model.ts` | The state machine: which screen each worker is on, the state word and colour, notifications. A port of the Objective-C Mac app's `evaluate`. |
| `ui/src/app.ts` | The window: renders the model, runs the buttons, keeps the tray menu in step. |
| `ui/src/menu.ts` | The tray menu, built from the model. |
| `ui/src/backend.ts` | Every call into Rust, and a stand-in for a plain browser (previews, screenshots, tests). |
| `ui/src/fake.ts` | Made-up data for every screen. |
| `ui/src/text.ts` | The words and numbers: times, sizes, the pairing message, the default description. |
| `src-tauri/src/lib.rs` | The commands the window calls; the tray; notifications, dialogs, clipboard, open at login. |
| `src-tauri/src/files.rs`, `paths.rs` | The worker folders and the files in them (`worker.json`, `requests/`, `approvals/`, `app.json`, `review.json`, command files). |
| `ui/src/setup.ts` | The first launch's words: the checklist, the runtime cards, the hub's red line, Needs attention. |
| `src-tauri/src/engine.rs` | Finding programs (`which`, `search_path`) and starting them with that PATH (`command`). Docker or Podman: the runtime check (ok, missing, not running, no permission, no compose, its own error), `compose up` and `down` with the output streamed to the log, the browser view's port, stale containers. |
| `src-tauri/src/install.rs` | Which runtimes this system can have and whether the app can install each (winget, Homebrew, apt/dnf/pacman through pkexec), starting Docker Engine, compose's package. |
| `src-tauri/src/hub.rs` | Whether the hub answers (`/config.json`), and the newest release. |
| `src-tauri/src/agents.rs` | The agents on this computer, read with their own client's `myous status --json`; accepting a pairing code as one of them. |
| `src-tauri/src/containers.rs` | Whether containers can run here at all (WSL 2 and virtualization on Windows); turning WSL on. |
| `src-tauri/src/direct.rs` | Workers without a container: finding the bundled worker, starting and stopping it. |
| `../build-worker.sh` | Builds that worker (the Go client) into `src-tauri/binaries/myous-worker-<target>`, where Tauri takes it from. |
| `src-tauri/icons/` | The app icons, made from the mark by `../icons/make-icons.sh` (don't edit them by hand); the tray icon is drawn by `trayIcon` in `ui/src/app.ts`, the same mark on a square in the state's colour. |

## Building

You need Node 22, Rust (stable) and Go (the toolchain `go/go.mod` pins;
the `go` command fetches it). On Linux also the system libraries Tauri
uses:

```sh
sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev build-essential file
```

Then, on the system you build for (Windows builds on Windows):

```sh
sh worker/build-worker.sh host worker/desktop/src-tauri/binaries   # the worker the app carries; any Rust build needs it
cd worker/desktop
npm ci
npm test                      # the state machine, the menu, the words
(cd src-tauri && cargo test)  # the folders, the runtime, the compose file, the worker's command line, the worker browser without a container
npx tauri build               # Linux: AppImage and deb; Windows: the NSIS installer
npx tauri dev                 # run it from source
```

`smoke/run.sh` starts the built AppImage and `.deb` under Xvfb in Docker
(on a Mac too) and checks that the window opens and renders; CI runs the
same `smoke/smoke.sh` after the Linux build (`smoke/README.md`).

On a Mac, `./make-mac.sh` builds the app (next section).

## On a Mac

The same app, built on macOS with the system's web view (WKWebView). The
Mac parts are behind `cfg(target_os = "macos")` in Rust and
`html.os-macos` in the window, so the Linux and Windows app is unchanged
by them. `MAC-PORT.md` has the details and what is still to come.

**Build and try it** (Node 22, Rust, Go and the Command Line Tools;
`make-mac.sh` builds the worker it carries itself):

```sh
cd worker/desktop
./make-mac.sh                 # build/myous.app for this Mac's architecture, signed ad hoc, and build/myous-<version>.dmg
./make-mac.sh --universal     # Apple Silicon and Intel in one app (rustup target add aarch64-apple-darwin x86_64-apple-darwin)
open build/myous.app
MYOUS_WORKER_HOME=/tmp/w MYOUS_FAKE_STATE=pair build/myous.app/Contents/MacOS/myous-desktop   # look without touching a real worker
python3 screens.py /tmp/screens --os macos                                                   # the previews at the Mac window's size
```

`make-mac.sh` runs `tauri build --bundles app --no-sign`, signs the
worker and then the app (`--sign "Developer ID Application: …"` for the
hardened runtime and a timestamp, else ad hoc), and packs the disk image
with `worker/mac/make-dmg.sh`; with `--key-id`, `--issuer` and `--key` it
also notarizes with `worker/mac/notarize.sh`. The release builds it
universal and signed (`.github/workflows/release.yml`, the `mac-app`
job). Inside the app the program is `Contents/MacOS/myous-desktop` (not
on PATH, so it never stands in for the client's `myous`), with
`myous-worker` next to it.

**What differs on a Mac:**

- A menu bar app (no Dock icon unless Settings turns one on); the menu
  bar icon is the mark in the state's colour, with the count of new
  requests next to it and on the Dock icon.
- The window is dressed as a Mac window (the system's text sizes,
  colours, push buttons, pop-up menus and striped table; dark mode, the
  accent colour and Increase Contrast follow System Settings), fits its
  content and isn't resizable. What the ⋯ menu holds elsewhere is in the
  app menu (Settings… on ⌘,), and Running has Open log.
- A question's notification has Allow and Refuse (UserNotifications),
  and open at login is a login item (SMAppService), as in the
  Objective-C app.
- The default worker name is the Computer Name.
- The worker's browser runs on the Mac under the sandbox
  (`sandbox-exec` with the Objective-C app's profile): Chrome, Edge,
  Brave or Chromium, with its profile in `<worker folder>/browser/<bundle
  id>` (where the Objective-C app kept it, so its logins carry over).
  For a worker in a container Set up asks where its browser should run,
  and Settings can change it. A worker without a container never uses
  the Mac's `/usr/bin/python3`.
  The runtime cards install with Homebrew.

## Looking at the screens without a worker

The window runs in any browser with made-up data:

```sh
npm run build:ui
python3 -m http.server -d ui/dist 8000   # then open http://localhost:8000/index.html?state=pair&os=windows
python3 screens.py /tmp/screens          # every state as a PNG (Playwright for Python), failing on any script error
python3 clicks.py                        # clicks the first-launch buttons and checks what each asks for (CI runs both)
```

`state` is one of checking, couldntcheck, setup, hubdown, noruntime,
installing, stoppedruntime, stoppedengine, nopermission, nocompose,
runtimeerror, nowsl, novirt, direct, directnobrowser, directnopython,
directnovenv, directnoplaywright, found, directinstalling, directoff,
directstarting, directrunning, directbrowserclosed, directbrowserlater,
starting, failed, pair, paired, running, paused, stopping, stopped,
approval, busy, agents, agent, pairlocal, workers, macbrowser (`screens.py`
renders all 40, and two of them dark too). The app itself takes the same names in
`MYOUS_FAKE_STATE`. In a plain browser the page's backend only records
what the buttons ask for (`window.myous.backend.calls`), and
`window.myous.backend.emit(event, payload)` plays what Rust would send (an
install's end, a log line), so the buttons can be clicked in tests.

Other environment variables, for tests: `MYOUS_WORKER_HOME` (another
worker folder), `MYOUS_COMPOSE_PROJECT` (the first worker's compose
project name, so a test never touches an installed worker's containers),
`MYOUS_DOCKER_BIN` (the runtime command; a missing path simulates a
computer without Docker), `MYOUS_AGENT_HOMES` (replaces the search for
agents' folders), `MYOUS_HUB` (the hub the app checks and asks for the
latest release),
`MYOUS_IMAGE_TAG` (the worker image tag instead of the app's version),
`MYOUS_WORKER_BIN` (another worker program to run without a
container, such as a local `go build` of `go/cmd/myous`; the app log
says when it is used), `MYOUS_BROWSER_BIN` (another Chromium for the
worker's browser without a container) and `MYOUS_APP_DATA` (the folder
for the app's Python environment instead of the system's).

The way without a container, end to end with the real Go worker, a local
hub and a Python agent (starts the worker as the app does, pairs the
agent, runs a read and a change the review asks about at "changes",
answered as the app answers, and stops it): `cargo test -- --ignored e2e`
in `src-tauri`. It builds and starts the hub from `../myoushq/hub` next
to this checkout (`MYOUS_HUB_SRC` elsewhere, or `MYOUS_E2E_HUB=<url>` for
one already running) and needs a Python with the client
(`MYOUS_PYTHON`, default `/tmp/myous-venv/bin/python`). With a Chromium
(`MYOUS_E2E_BROWSER`, else Playwright's Chrome for Testing in its cache)
it also starts the worker's browser as the app does (headless; once the
worker runs, as "Open browser" does), checks that the worker's `help`
names it (read from `browser.json`) and the Python with Playwright
(`MYOUS_E2E_PW_PYTHON`, default `/tmp/pw-venv/bin/python`), has the
agent run a Playwright script through `myous exec` that reads the start
page's title over the browser's port, brings it forward as Show browser
does, and checks it ends with the worker. `cargo test -- --ignored
playwright_install` makes the Python environment and installs Playwright
for real in a temporary folder (`MYOUS_E2E_SYSTEM_PYTHON` names the
Python 3.10+ to make it with).

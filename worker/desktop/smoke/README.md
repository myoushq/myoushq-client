# Smoke test: myous for Linux under Xvfb

Starts the built app (the AppImage and the `.deb`) on a virtual X display
and checks that its window opens and shows something. No Linux desktop
needed: `run.sh` does it in Docker, so it runs on a Mac too. CI runs
`smoke.sh` directly on the runner after building the bundles (the
`desktop-app-linux` job in `.github/workflows/ci.yml`).

## What it checks

`smoke.sh` starts the app under Xvfb (1280x800) with
`WEBKIT_DISABLE_COMPOSITING_MODE=1` and `WEBKIT_DISABLE_DMABUF_RENDERER=1`
(no GPU there), then:

- the window "myous" (class `myous-desktop`) maps;
- the app is still running at the last screenshot, with no tray host
  (bare Xvfb has none; the app must not need one);
- the window area has more than 20 colours (a blank web view has one or
  two), so the page really rendered;
- with `CLICK`, it clicks once and takes another screenshot.

It exits non-zero when a check fails. In the container `run.sh` builds,
there is no container runtime, so the first screen is "Get a container
runtime" (✗ Container runtime: none found, a "Get …" button per runtime,
"▸ Run without a container"), and the click opens "Run without a
container" with its warning. It does not press the button under it (that
would start a worker against the hub).

The `.deb` target also records what the package installs
(`/usr/bin/myous-desktop`, `/usr/bin/myous-worker`, the icons and
`/usr/share/applications/myous.desktop`).

## Running it on a Mac (or any machine with Docker)

```sh
worker/desktop/smoke/run.sh 0.7.3                     # a release (downloads with gh, checks SHA256SUMS)
worker/desktop/smoke/run.sh --appimage path/to/myous.AppImage --deb path/to/myous.deb
worker/desktop/smoke/run.sh 0.7.3 --out /tmp/smoke --keep   # results elsewhere, keep the images
```

The images are `linux/amd64` (emulated on Apple Silicon: slower, works).
The first build pulls `debian:bookworm-slim` and the WebKitGTK libraries
(about 4 minutes on an Intel Mac); a run takes under a minute. The images
are about 1.1 GB (AppImage) and 0.7 GB (`.deb`); `run.sh` removes them
when it finishes unless `--keep` is given.

## Results

In `--out` (default `./myous-smoke-out`), per target (`appimage`, `deb`):

| File | What |
|---|---|
| `<t>-3s.png`, `<t>-10s.png` | the screen 3 s and 10 s after the window mapped |
| `<t>-click.png` | after the click |
| `<t>-result.txt` | the checks: mapped and how fast, alive, colours, `ok=yes/no` |
| `<t>-app.log` | the app's stdout and stderr (empty when all is well) |
| `<t>-windows.txt` | `xwininfo -root -tree` |
| `<t>-ps.txt` | the processes and their memory |
| `deb-files.txt`, `deb-install.log` | what the `.deb` installs, and apt's output |

## Seen so far

- v0.7.3 (2026-10-13): both bundles render the same screen pixel for
  pixel; the window maps 1-2 s after start (5 s on the first, cold run);
  nothing on stdout or stderr, no tray or WebKit warnings.
- On the first, cold run once, the "Get …" buttons were still missing
  about 15 s after start (the list comes from `runtime_options`, which
  asks the package manager); later runs had them within a second. The
  card now says "Looking for ways to install one…" while it waits.

#!/usr/bin/env python3
"""Renders every screen of the window in a headless browser, from ui/dist
with made-up data (the page's own preview mode: no Tauri, no worker), and
fails on any script error. The screenshots and the tray menu of each go to
OUTDIR for a look, and the dialogs in DIALOGS as screens of their own.
Needs Playwright for Python with Chromium.

    npm run build:ui && python3 screens.py OUTDIR [--os windows|macos] [--engine webkit]

--engine webkit draws them as a Mac's web view (WKWebView) does. On macOS
each screen is drawn at the size the window takes there (the page's
fitWindow: 560 wide, as high as its content).
"""

import functools
import http.server
import json
import sys
import threading
from pathlib import Path

from playwright.sync_api import sync_playwright

STATES = ["checking", "couldntcheck", "setup", "hubdown", "noruntime", "installing", "stoppedruntime", "stoppedengine",
          "nopermission", "nocompose", "runtimeerror", "nowsl", "novirt", "direct", "directnobrowser", "directnopython", "directnovenv",
          "directnoplaywright", "found", "directinstalling", "directoff", "directstarting", "directrunning", "directbrowserclosed", "directbrowserlater", "starting",
          "failed", "pair", "paired", "running", "macbrowser", "paused", "stopping", "stopped", "approval", "busy", "agents", "agent", "pairlocal",
          "workers"]

# Dialogs drawn as screens: (name, state, the made-up containers kept (None: all), the menu entry
# that opens it). Remove stale containers with one container of each kind (two workers), and with
# none; Settings.
DIALOGS = [("stale", "workers", None, "m:Remove stale containers…"), ("stale-none", "running", [], "m:Remove stale containers…"),
           ("settings", "running", None, "m:Settings…")]


def main() -> int:
    out = Path(sys.argv[1] if len(sys.argv) > 1 else "screens")
    os_name = sys.argv[sys.argv.index("--os") + 1] if "--os" in sys.argv else "linux"
    engine = sys.argv[sys.argv.index("--engine") + 1] if "--engine" in sys.argv else "chromium"
    out.mkdir(parents=True, exist_ok=True)
    dist = Path(__file__).parent / "ui" / "dist"
    class Quiet(http.server.SimpleHTTPRequestHandler):
        def log_message(self, *args):
            pass

    handler = functools.partial(Quiet, directory=str(dist))
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    port = server.server_address[1]
    failures = []
    mac = os_name == "macos"
    size = {"width": 560, "height": 420} if mac else {"width": 640, "height": 700}

    def fit(page) -> None:
        # The size the page asked for, until it stops changing (a wider window wraps less).
        last = None
        for _ in range(10):
            page.wait_for_function("window.myous.backend.calls.some(c => c[0] === 'fitWindow')")
            page.wait_for_timeout(150)
            w, h = page.evaluate("window.myous.backend.calls.filter(c => c[0] === 'fitWindow').pop()[1].slice(0, 2)")
            if (w, h) == last:
                return
            last = (w, h)
            page.set_viewport_size({"width": w, "height": h})

    with sync_playwright() as p:
        browser = getattr(p, engine).launch()
        for scheme in ("light", "dark"):
            page = browser.new_page(viewport=size, color_scheme=scheme)
            errors: list[str] = []
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.on("console", lambda msg: errors.append(msg.text) if msg.type == "error" else None)
            for state in STATES if scheme == "light" else ["running", "pair"]:
                errors.clear()
                page.goto(f"http://127.0.0.1:{port}/index.html?state={state}&os={os_name}")
                page.wait_for_function("window.myous && window.myous.model")
                if mac:
                    fit(page)
                elif state == "workers":
                    page.set_viewport_size({"width": 840, "height": 700})
                name = f"{os_name}-{state}" + ("" if scheme == "light" else "-dark")
                page.screenshot(path=str(out / f"{name}.png"), full_page=True)
                tray = page.evaluate("window.myous.backend.tray")
                (out / f"{name}-tray.json").write_text(json.dumps(tray and tray["items"], indent=1))
                if errors:
                    failures.append(f"{state}: {errors}")
                page.set_viewport_size(size)
            for name, state, keep, entry in DIALOGS:
                errors.clear()
                page.goto(f"http://127.0.0.1:{port}/index.html?state={state}&os={os_name}")
                page.wait_for_function("window.myous && window.myous.model")
                if keep is not None:
                    page.evaluate(f"(b => b.containers = b.containers.filter(c => {json.dumps(keep)}.includes(c.name)))(window.myous.backend)")
                if not mac and state == "workers":
                    page.set_viewport_size({"width": 840, "height": 700})
                page.evaluate(f"window.myous.backend.emit('tray', {json.dumps(entry)})")
                page.wait_for_selector("#dlg[open]")
                if mac:
                    fit(page)
                file = f"{os_name}-{name}" + ("" if scheme == "light" else "-dark")
                page.screenshot(path=str(out / f"{file}.png"), full_page=True)
                if errors:
                    failures.append(f"{name}: {errors}")
                page.set_viewport_size(size)
            page.close()
        browser.close()
    server.shutdown()
    for f in failures:
        print("FAIL", f)
    print(f"{len(STATES) + 2 + 2 * len(DIALOGS)} screens in {out}" if not failures else f"{len(failures)} failed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())

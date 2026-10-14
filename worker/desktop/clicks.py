#!/usr/bin/env python3
"""Clicks the window's first-launch buttons in a headless browser, from
ui/dist with made-up data (the page's own preview mode: its backend only
records what each button asks of Rust, and `emit` plays what Rust would
send back), and fails on a wrong result or any script error. Needs
Playwright for Python with Chromium.

    npm run build:ui && python3 clicks.py
"""

import functools
import http.server
import json
import sys
import threading
from pathlib import Path

from playwright.sync_api import sync_playwright

DOCKER_DESKTOP = "https://www.docker.com/products/docker-desktop/"


def main() -> int:
    dist = Path(__file__).parent / "ui" / "dist"

    class Quiet(http.server.SimpleHTTPRequestHandler):
        def log_message(self, *args):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Quiet, directory=str(dist)))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    port = server.server_address[1]
    failures: list[str] = []

    def check(cond: bool, what: str) -> None:
        print(("ok  " if cond else "FAIL"), what)
        if not cond:
            failures.append(what)

    with sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page(viewport={"width": 640, "height": 700})
        errors: list[str] = []
        page.on("pageerror", lambda e: errors.append(str(e)))

        def go(state: str, os_name: str = "linux") -> None:
            page.goto(f"http://127.0.0.1:{port}/index.html?state={state}&os={os_name}")
            page.wait_for_function("window.myous && window.myous.model")

        def calls() -> list:
            return page.evaluate("window.myous.backend.calls.map(c => [c[0], c[1]])")

        def names() -> list[str]:
            return [c[0] for c in calls()]

        def emit(event: str, payload: dict) -> None:
            page.evaluate(f"window.myous.backend.emit({json.dumps(event)}, {json.dumps(payload)})")

        text = page.inner_text

        # Checking: the checklist alone, the form once the answers are in.
        go("checking")
        check(page.is_hidden("#setup-form") and page.is_visible("#setup-checking") and text("#setup-checks").count("⟳") == 4,
              "checking: four ⟳ lines, no form yet")

        # Couldn't check: the form anyway; Retry asks again and the answer replaces it.
        go("couldntcheck")
        check(page.is_visible("#setup-form"), "couldn't check: the form shows anyway")
        page.click("#setup-checks button[data-retry=runtime]")
        page.wait_for_function("document.querySelector('#setup-checks').innerText.includes('Container runtime: Docker Desktop')")
        check("probeRuntime" in names(), "Retry on the runtime runs the check; its answer is a tick")
        page.click("#setup-checks button[data-retry=hub]")
        page.wait_for_function("document.querySelector('#setup-checks').innerText.includes('myoushq.com answers')")
        check("hubCheck" in names(), "Retry on myoushq.com runs the check")

        # The hub's red line: Start stays enabled; Retry clears it when the hub answers.
        go("hubdown")
        check("Can't reach myoushq.com" in text("#banner") and page.is_enabled("#setup-start"), "hub down: red line, Start enabled")
        page.click("#banner-button")
        page.wait_for_selector("#banner", state="hidden")
        check("hubCheck" in names(), "the red line's Retry checks again and the line goes")

        # Get a container runtime (Linux): Install through pkexec; the card leaves when the runtime answers.
        go("noruntime")
        check(text("#runtime-title") == "Get a container runtime", "no runtime: Get a container runtime")
        page.click("#runtime-list button[data-install=docker-engine]")
        check(["runtimeInstall", ["docker-engine", "Docker"]] in calls(), "Install Docker Engine runs the install")
        check("Installing Docker Engine" in text("#runtime-install") and page.is_hidden("#runtime-list"),
              "installing: the line, the buttons gone")
        emit("app-log", {"home": "/home/max/.myous-worker", "line": "Setting up docker.io (26.1.3)"})
        emit("install-done", {"id": "docker-engine", "status": 0})
        page.wait_for_selector("#card-setup:not([hidden])")
        check("probeRuntime" in names(), "install done: the runtime is checked and the card leaves for Set up")
        go("noruntime")
        page.evaluate("window.myous.backend.probeResult = {state: 'missing', name: 'Docker', detail: ''}")
        page.click("#runtime-list button[data-install=podman]")
        emit("app-log", {"home": "/home/max/.myous-worker", "line": "Error executing command as another user: Not authorized"})
        emit("install-done", {"id": "podman", "status": 127})
        page.wait_for_function("document.querySelector('#runtime-install').innerText.includes('didn')")
        check("Not authorized" in text("#runtime-install") and page.is_visible("#runtime-list"), "install failed: why, and the buttons back")
        page.click(f"#runtime-list button[data-url='{DOCKER_DESKTOP}']")
        check(["openUrl", [DOCKER_DESKTOP]] in calls(), "Docker Desktop: its page")
        page.click("#direct-toggle")
        check(page.is_visible("#direct-use"), "the not-recommended way is folded under the toggle")

        # Windows: winget installs.
        go("noruntime", "windows")
        page.click("#runtime-list button[data-install=rancher-desktop]")
        check(["runtimeInstall", ["rancher-desktop", "Docker"]] in calls(), "Windows: Install Rancher Desktop runs winget")

        # Not running: the desktop app opens; Docker Engine starts through pkexec.
        go("stoppedruntime")
        page.click("#runtime-fix")
        check(["launchRuntime", ["Docker Desktop"]] in calls(), "Open Docker Desktop")
        go("stoppedengine")
        page.click("#runtime-fix")
        check(["runtimeInstall", ["start", "Docker Engine"]] in calls() and "Starting Docker Engine" in text("#runtime-install"),
              "Start Docker Engine runs pkexec systemctl")

        # Not allowed: the line, copied.
        go("nopermission")
        page.click("#runtime-copy")
        check(["copyText", ["sudo usermod -aG docker $USER"]] in calls(), "permission: Copy copies the line")

        # Compose missing: installed here, or its page.
        go("nocompose")
        page.click("#runtime-fix")
        check(["runtimeInstall", ["compose", "Docker Engine"]] in calls(), "compose: Install it")
        page.click("#runtime-link")
        check(["openUrl", ["https://docs.docker.com/compose/install/linux/"]] in calls(), "compose: how to install it")

        # The runtime's own error, as it said it.
        go("runtimeerror")
        check("unable to start" in text("#runtime-lines"), "error: the runtime's words shown")

        # Needs attention: Show log; Try again starts again without Stop.
        go("failed")
        check(page.is_visible("#card-attention") and page.is_hidden("#banner-button") and "no such host" in text("#attention-log"),
              "failed: Needs attention with the last lines")
        page.click("#attention-show-log")
        page.wait_for_selector("#dlg[open]")
        check("myous log" in text("#dlg"), "Show log opens the log")
        page.keyboard.press("Escape")
        page.click("#attention-retry")
        page.wait_for_selector("#card-starting:not([hidden])")
        check("composeRun" in names() and "hubCheck" in names(), "Try again starts it (compose up) and checks the hub")

        # Windows without virtualization: straight on to Set up without a container.
        go("novirt", "windows")
        page.click("#direct-use")
        page.wait_for_selector("#card-setup:not([hidden])")
        check(page.is_visible("#setup-form") and "as you" in text("#setup-runtime") and "Container runtime" not in text("#setup-checks"),
              "no virtualization: Set up without a container, no container lines")
        # Without a container: the worker's browser on this computer.
        home = "/home/max/.myous-worker"
        go("direct")
        check("✓ Worker browser: Chrome, with Python and Playwright" in text("#setup-wbrowser")
              and "No worker browser" not in text("#setup-runtime"), "direct: the browser row ticks, the facts don't say no browser")
        check("Its browser is where I log into sites" in page.input_value("#setup-desc"), "the default description names the browser")
        page.click("#setup-wbrowser button[data-browser=toggle]")
        check("Its browser" not in page.input_value("#setup-desc"), "without a browser the description doesn't")
        check("○ No worker browser" in text("#setup-wbrowser") and "No worker browser" in text("#setup-runtime")
              and "Use a worker browser" in text("#setup-wbrowser"), "Continue without a browser: off, said in the facts")
        page.click("#setup-start")
        page.wait_for_selector("#card-starting:not([hidden])")
        started = [c for c in calls() if c[0] == "directStart"]
        check(started and started[-1][1][:2] == [home, "maxs-pc"] and started[-1][1][3] is False
              and "Starting the browser" not in text("#card-starting"), "started without a browser when it's off")
        go("direct")
        page.click("#setup-start")
        page.wait_for_selector("#card-starting:not([hidden])")
        started = [c for c in calls() if c[0] == "directStart"]
        check(started and started[-1][1][3] is True and "⟳ Starting the browser" in text("#card-starting"),
              "Start with the browser on: the app starts it, the Starting card says so")
        go("directnoplaywright")
        check("○ Worker browser: install Playwright" in text("#setup-wbrowser"), "Python there: install Playwright")
        page.evaluate("window.myous.backend.browserResult = {...window.myous.backend.browserResult, playwright: '', "
                      "python: {path: '/usr/bin/python3', version: '3.12', venv: true}}")
        page.click("#setup-wbrowser button[data-browser=install]")
        check(["browserInstall", [home]] in calls() and "Installing Playwright" in text("#setup-wbrowser"),
              "Install Playwright runs the install; the row says so")
        emit("app-log", {"home": home, "line": "ERROR: No matching distribution found for playwright==1.63.0"})
        emit("install-done", {"id": "playwright", "status": 1})
        page.wait_for_function("document.querySelector('#setup-wbrowser').innerText.includes('didn')")
        check("No matching distribution" in text("#setup-wbrowser") and page.is_visible("#setup-wbrowser button[data-browser=install]"),
              "install failed: why, and the button again")
        page.evaluate("window.myous.backend.browserResult = {...window.myous.backend.browserResult, playwright: '1.63.0'}")
        page.click("#setup-wbrowser button[data-browser=install]")
        emit("install-done", {"id": "playwright", "status": 0})
        page.wait_for_function("document.querySelector('#setup-wbrowser').innerText.includes('with Python and Playwright')")
        check("browserCheck" in names(), "install done: checked again, the row ticks")
        check("✓ Playwright is installed. The browser is ready." in text("#setup-wbrowser .found")
              and page.is_visible("#setup-wbrowser p.advanced"), "ready: the row flashes and says what it found")
        go("directnopython", "windows")
        page.click("#setup-wbrowser button[data-browser=get]")
        check(["openUrl", ["https://www.python.org/downloads/windows/"]] in calls(), "Windows: Get Python opens python.org")
        go("directnopython")
        page.click("#setup-wbrowser button[data-browser=copy]")
        check(["copyText", ["sudo apt install python3 python3-venv"]] in calls(), "Linux: the package line, copied")
        go("directnobrowser", "windows")
        page.click("#setup-wbrowser button[data-url='https://www.microsoft.com/edge/download']")
        check(["openUrl", ["https://www.microsoft.com/edge/download"]] in calls(), "no browser: Get Edge")
        go("directrunning")
        check(page.is_visible("#card-browser") and text("#browser-open") == "Show browser" and "Browser on this computer" in text("#head-facts"),
              "running with its browser: the Browser card, the header fact")
        tray = page.evaluate("window.myous.backend.tray.items.map(i => i.label)")
        check("Show browser" in tray, "the tray has Show browser")
        page.click("#browser-open")
        check(["browserShow", [home, "Max's PC"]] in calls(), "Show browser brings it forward")
        go("directbrowserclosed")
        check(text("#browser-open") == "Open browser" and "closed" in text("#browser-text"), "closed: Open browser")
        page.click("#browser-open")
        check(["browserShow", [home, "Max's PC"]] in calls(), "Open browser starts it again")
        go("directbrowserlater")
        check(text("#browser-open") == "Open browser" and "runs without its browser" in text("#browser-text")
              and "estart" not in text("#browser-text"), "started without it: Open browser, no restart")
        page.click("#browser-open")
        check(["browserShow", [home, "Max's PC"]] in calls(), "Open browser starts it while the worker runs")
        page.wait_for_function("document.querySelector('#browser-text').innerText.includes('closed')")
        check("closed" in text("#browser-text"), "then it is the worker's browser (starting; not up in the preview)")

        # A Mac: Settings has the Dock icon, applied at once on Save; the other systems have none.
        def settings(os_name: str) -> None:
            go("running", os_name)
            page.click("#more")
            page.click("#more-menu button:has-text('Settings…')")

        settings("linux")
        check("Dock" not in text("#dlg") and "setCounts" not in names(), "Linux: no Dock setting, no counts")
        page.click("#dlg button:has-text('Cancel')")
        settings("macos")
        page.click("#dlg label:has-text('Dock') input")
        page.click("#dlg button:has-text('Save')")
        check(["setDock", [True]] in calls(), "a Mac: the Dock setting applies at once")

        check(not errors, f"no script errors {errors}")
        browser.close()
    server.shutdown()
    print(f"{len(failures)} failed" if failures else "every click did what it should")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())

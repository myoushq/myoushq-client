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

        # A request that expired (sent while the worker was off, run never): the word and why, in its row and its dialog.
        go("running")
        row = page.locator("#requests tbody tr", has_text="quotes.py --all")
        check(row.locator("td").last.inner_text() == "expired" and "— sent 3 hours ago, its ttl is 150 seconds; not run" in row.inner_text()
              and "expired: sent" not in row.inner_text(), "an expired request: \"expired\", and why")
        row.click()
        page.wait_for_selector("#dlg[open]")
        check("Expired: sent 3 hours ago, its ttl is 150 seconds; not run" in text("#dlg") and "Exit" not in text("#dlg"),
              "its dialog: expired, and why")
        page.keyboard.press("Escape")
        page.wait_for_selector("#dlg", state="hidden")

        # A dialog opened while another is open replaces it and stays: Settings, then Show app log from the tray.
        go("running")
        emit("tray", "m:Settings…")
        page.wait_for_selector("#dlg[open]")
        emit("tray", "m:Show app log")
        page.wait_for_timeout(300)
        check(page.evaluate("document.getElementById('dlg').open") and text("#dlg h2") == "myous log",
              "Show app log from the tray while Settings is open: the log shows")
        page.keyboard.press("Escape")
        page.wait_for_selector("#dlg", state="hidden")
        check(not page.evaluate("document.getElementById('dlg').open"), "Escape closes it")

        # A Mac: Settings has the Dock icon, applied at once on Save; the other systems have none.
        def settings(os_name: str) -> None:
            go("running", os_name)
            page.click("#more")
            page.click("#more-menu button:has-text('Settings…')")

        settings("linux")
        check("Dock" not in text("#dlg") and "setCounts" not in names(), "Linux: no Dock setting, no counts")
        check(not {"fitWindow", "setAppMenu"} & set(names()) and page.is_hidden("#requests-log-row"),
              "Linux: the window keeps its size, no app menu, the log in ⋯")
        page.click("#dlg button:has-text('Cancel')")
        # A Mac: no ⋯; its entries are the app menu's (a click comes back as a "tray" event), Open log is on Running.
        go("running", "macos")
        app_menu = page.evaluate("window.myous.backend.appMenu")
        labels = [i.get("label") for i in app_menu]
        check(page.is_hidden("#more") and labels[:2] == ["Settings…", "Check for updates…"]
              and app_menu[0].get("accelerator") == "CmdOrCtrl+," and "Show app log" in labels and "Quit myous" not in labels,
              "a Mac: no ⋯, its entries in the app menu, Settings… with ⌘,")
        fits = [c[1] for c in calls() if c[0] == "fitWindow"]
        check(bool(fits) and fits[-1][0] == 560 and 320 <= fits[-1][1] <= 760, f"a Mac: the window fits its content {fits[-1:]}")
        page.click("#requests-log")
        page.wait_for_selector("#dlg[open]")
        check("myous log" in text("#dlg"), "a Mac: Open log on Running opens the log")
        page.keyboard.press("Escape")
        page.wait_for_selector("#dlg", state="hidden")
        settings_id = next(i["id"] for i in app_menu if i.get("label") == "Settings…")
        # The window was closed (hidden): Settings from the tray's menu brings it back before the dialog.
        emit("window", False)
        shows = names().count("showWindow")
        emit("tray", settings_id)
        page.wait_for_selector("#dlg[open]")
        check(names().count("showWindow") > shows, "Settings from the tray shows the hidden window first")
        page.click("#dlg label:has-text('Dock') input")
        page.click("#dlg button:has-text('Save')")
        check(["setDock", [True]] in calls(), "a Mac: the Dock setting applies at once")
        # The window as it runs (no made-up state): it rereads the worker folder every two seconds, also
        # while Settings is open; Save keeps every app-wide setting, the Dock too, in one write of app.json.
        go("", "macos")
        emit("tray", "m:Settings…")
        page.wait_for_selector("#dlg[open]")
        reads = names().count("readWorkers")
        page.wait_for_function(f"window.myous.backend.calls.filter(c => c[0] === 'readWorkers').length > {reads}")
        page.fill("#dlg input:not([type])", "Max's Mac mini")
        for label in ["when a worker pairs", "waits for my answer", "when a request is refused", "stops on its own",
                      "a new version is available", "Notify me", "Check for a new version daily", "Show the agents", "Dock"]:
            page.click(f"#dlg label:has-text({json.dumps(label)}) input")
        writes = names().count("writeConfig")
        page.click("#dlg button:has-text('Save')")
        page.wait_for_selector("#dlg", state="hidden")
        page.wait_for_timeout(300)
        saved = page.evaluate("window.myous.backend.files[0].config")
        want = {"name": "Max's Mac mini", "notifications": False, "auto_update": False, "show_agents": False, "dock": True,
                "notify": {k: False for k in ["paired", "approval", "refused", "stopped", "update"]}}
        check({k: saved.get(k) for k in want} == want and names().count("writeConfig") == writes + 1 and ["setDock", [True]] in calls(),
              f"Settings saved after a reread: every app-wide setting and the Dock, one write {saved}")

        # The window as it runs, with two workers (the first just paired, the second running).
        def two_workers(os_name: str, first: str = "{}", second: str = "{}") -> list[str]:
            go("", os_name)
            page.evaluate(f"""(b => {{
                const f = b.files[0], now = Date.now() / 1000;
                b.files = [{{...f, status: {{...f.status, paired: {{alias: "Max's Muse", verify: "4821", at: now}}}}, ...{first}}},
                    {{...f, home: f.home + "-2", folder: ".myous-worker-2", is_default: false, project: "myous-worker-2",
                     config: {{name: "Codex's worker"}}, status: {{phase: "running", alias: "Codex's worker", contacts: 1}}, ...{second}}}];
            }})(window.myous.backend)""")
            page.wait_for_function("window.myous.model.workers.length === 2 && window.myous.model.workers.every(w => w.evaluated)")
            return page.evaluate("window.myous.backend.files.map(f => f.home)")

        # Unpair, answered after the window moved on to the other worker (one that came to need
        # attention), unpairs the worker it was pressed on.
        homes = two_workers("linux")
        page.wait_for_selector("#card-paired:not([hidden])")
        page.click("#unpair")
        page.wait_for_selector("#dlg[open]")
        page.evaluate("(m => { m.current = m.workers[1]; })(window.myous.model)")
        page.click("#dlg button:has-text('Unpair')")
        page.wait_for_selector("#dlg", state="hidden")
        page.wait_for_function("window.myous.backend.calls.some(c => c[0] === 'sendCommand')")
        sent = [c[1] for c in calls() if c[0] == "sendCommand"]
        check(sent == [[homes[0], "unpair"]], f"Unpair unpairs the worker it was pressed on, not the one shown by then {sent}")

        # A Mac's worker in a container: Set up asks where its browser runs, as myous for Mac does.
        def env_of_start() -> dict:
            return next(c[1][4] for c in reversed(calls()) if c[0] == "composeRun")

        go("setup", "macos")
        mac_home = "/Users/max/.myous-worker"
        options = page.eval_on_selector_all("#setup-where option", "os => os.map(o => o.textContent)")
        check(page.is_visible("#setup-macbrowser") and page.is_hidden("#setup-browser") and page.is_hidden("#setup-get")
              and options == ["On this Mac, in Google Chrome (recommended)", "On this Mac, in Microsoft Edge", "In the container"]
              and page.eval_on_selector("#setup-where", "s => s.selectedIndex") == 0
              and "kept to the worker's folder by a sandbox" in text("#setup-where-hint"),
              "a Mac's Set up: where its browser runs, Google Chrome recommended")
        page.select_option("#setup-where", index=1)
        page.click("#setup-start")
        page.wait_for_function("window.myous.backend.calls.some(c => c[0] === 'composeRun')")
        check(env_of_start().get("MYOUS_BROWSER") == "host"
              and ["macBrowserEnsure", [mac_home, "Max's Mac", "com.microsoft.edgemac", True]] in calls(),
              "On this Mac, in Microsoft Edge: MYOUS_BROWSER=host, Edge started (hidden, by default)")
        go("setup", "macos")
        page.select_option("#setup-where", index=2)
        check("Some sites take it for a bot" in text("#setup-where-hint"), "In the container: its line")
        page.click("#setup-start")
        page.wait_for_function("window.myous.backend.calls.some(c => c[0] === 'composeRun')")
        check(env_of_start().get("MYOUS_BROWSER") == "container" and ["browserStop", [mac_home]] in calls()
              and "macBrowserEnsure" not in names(), "In the container: MYOUS_BROWSER=container, none on the Mac")
        go("setup")
        check(page.is_hidden("#setup-macbrowser") and page.is_visible("#setup-browser"), "Linux: no browser question")
        page.click("#setup-start")
        page.wait_for_function("window.myous.backend.calls.some(c => c[0] === 'composeRun')")
        check(env_of_start().get("MYOUS_BROWSER") == "container"
              and not {"macBrowserEnsure", "browserStop", "macBrowsers"} & set(names()),
              "Linux: MYOUS_BROWSER=container, nothing of the Mac's")
        # Running with its browser on the Mac: Show browser; Settings has the choice and "hidden".
        go("macbrowser", "macos")
        check(text("#browser-open") == "Show browser" and "uses Google Chrome on this Mac" in text("#browser-text"),
              "a Mac: the Browser card names Google Chrome on this Mac")
        page.click("#browser-open")
        check(["macBrowserShow", [mac_home, "Max's Mac", "com.google.Chrome"]] in calls(), "Show browser brings it forward")
        emit("tray", next(i["id"] for i in page.evaluate("window.myous.backend.appMenu") if i.get("label") == "Settings…"))
        page.wait_for_selector("#dlg[open]")
        selects = page.eval_on_selector_all("#dlg select", "ss => ss.map(s => s.options[s.selectedIndex].textContent)")
        check("On this Mac, in Google Chrome (recommended)" in selects and page.is_visible("#dlg label:has-text('Start this worker')"),
              "a Mac's Settings: its browser and \"Start this worker's browser hidden\"")
        page.click("#dlg label:has-text('Start this worker') input")
        page.click("#dlg button:has-text('Save')")
        check(["macBrowserHidden", [mac_home, False]] in calls(), "not hidden: applied at once")
        # The window as it runs, a worker with its browser on the Mac: a Stop while that browser is being
        # started (the start held here) ends it once it has started.
        go("", "macos")
        page.evaluate("""(b => {
            b.files[0].config.browser = "mac";
            const ensure = b.macBrowserEnsure.bind(b);
            b.macBrowserEnsure = (...a) => new Promise((done) => { window.releaseEnsure = () => done(ensure(...a)); });
        })(window.myous.backend)""")
        page.wait_for_function("typeof window.releaseEnsure === 'function'")
        emit("tray", f"w:{mac_home}:Stop")
        page.wait_for_function("window.myous.backend.calls.some(c => c[0] === 'composeRun')")
        stops = names().count("browserStop")
        page.evaluate("window.releaseEnsure()")
        page.wait_for_timeout(500)
        check(stops == 1 and names().count("browserStop") == 2, "Stop during the browser's start: ended by Stop, and again once it started")
        settings("linux")
        check("Its browser" not in text("#dlg") and "hidden" not in text("#dlg"), "Linux: no browser choice in Settings")
        page.click("#dlg button:has-text('Cancel')")

        # Remove stale containers: a dialog that lists the worker containers and asks which go.
        stale_id = "m:Remove stale containers…"

        def stale_dialog(state: str, keep: list[str] | None = None) -> dict:
            go(state)
            if keep is not None:
                page.evaluate(f"(b => b.containers = b.containers.filter(c => {json.dumps(keep)}.includes(c.name)))(window.myous.backend)")
            ids = {c["name"]: c["id"] for c in page.evaluate("window.myous.backend.containers")}
            emit("tray", stale_id)
            page.wait_for_selector("#dlg[open]")
            return ids

        def removed() -> list:
            return [c[1] for c in calls() if c[0] == "removeContainers"]

        # Two workers: one container of each kind. The default removes just the stopped ones, with a plain rm.
        ids = stale_dialog("workers")
        dlg = text("#dlg")
        check("Remove stale containers" in dlg and all(n in dlg for n in ids) and "Just the stopped ones (2)" in dlg
              and "All, running ones too (3)" in dlg and page.is_checked("#dlg label:has-text('Just the stopped') input"),
              "the dialog lists every worker container; just the stopped ones by default")
        check("Also remove Max's PC's container: it runs myous 0.6.0, this app is 0.6.1. The worker stops; Start runs it on 0.6.1." in dlg
              and not page.is_checked("#dlg label:has-text('Also remove') input")
              and "Max's PC for Codex's container is kept: it runs this app's version." in dlg
              and "Also remove Max's PC for Codex" not in dlg, "this app's older worker is asked about, unticked; its current one is kept")
        page.click("#dlg button:has-text('Remove')")
        page.wait_for_selector("#dlg", state="hidden")
        check(removed() == [[[ids["myous-worker-3-worker-1"], ids["myous-worker"]], []]], "Remove: the stopped ones, without force")
        # All, and the older worker ticked (from the ⋯ menu this time): the others and it, with force; the worker
        # shows Stopped a minute on, not "stopped on its own".
        go("workers")
        ids = {c["name"]: c["id"] for c in page.evaluate("window.myous.backend.containers")}
        page.click("#more")
        page.click("#more-menu button:has-text('Remove stale containers…')")
        page.wait_for_selector("#dlg[open]")
        page.click("#dlg label:has-text('All, running ones too') input")
        page.click("#dlg label:has-text('Also remove') input")
        # The runtime stops each running container before removing it (ten seconds or so each): the
        # removal is held while the window looks every two seconds for a minute, the first worker's
        # status unwritten (its container stopping), the second's written.
        page.evaluate("(b => { const remove = b.removeContainers.bind(b); "
                      "b.removeContainers = (s, r) => new Promise((done) => { window.releaseRemove = () => done(remove(s, r)); }); })(window.myous.backend)")
        page.click("#dlg button:has-text('Remove')")
        page.wait_for_selector("#dlg", state="hidden")
        page.wait_for_function("typeof window.releaseRemove === 'function'")
        page.evaluate("""(() => { const m = window.myous.model; let t = m.now(); m.now = () => t;
            for (let i = 0; i < 30; i++) { t += 2; m.workers[1].statusModified = t; m.workers.forEach((w) => m.evaluate(w)); } })()""")
        meanwhile = page.evaluate("(({screen, attention}) => ({screen, attention}))(window.myous.model.workers[0])")
        check(meanwhile == {"screen": "stopped", "attention": ""}, f"while its container is removed: stopping, then Stopped, not on its own {meanwhile}")
        page.evaluate("window.releaseRemove()")
        page.wait_for_function("window.myous.backend.calls.some(c => c[0] === 'removeContainers')")
        check(removed() == [[[ids["myous-worker-3-worker-1"], ids["myous-worker"]],
                             [ids["myous-desktop-e2e-1791700000-worker-1"], ids["myous-worker-worker-1"]]]],
              "All with the older worker ticked: the others and it, with force")
        page.wait_for_function("window.myous.model.workers[0].screen === 'stopped'")
        first = page.evaluate("(({stateWord, attention}) => ({stateWord, attention}))(window.myous.model.workers[0])")
        check(first == {"stateWord": "Stopped", "attention": ""} and page.is_visible("#stopped-start") and page.is_hidden("#banner"),
              f"its worker shows Stopped with Start, no Needs attention {first}")
        tray = [i.get("label") for i in page.evaluate("window.myous.backend.tray.items")]
        check("Max's PC · Stopped" in tray, f"the tray says Stopped too {tray}")
        # Only this app's older worker running: nothing to remove until it is ticked.
        stale_dialog("running", ["myous-worker-worker-1"])
        check("Just the stopped ones (0)" in text("#dlg") and "All, running ones too (0)" in text("#dlg")
              and page.is_disabled("#stale-remove"), "nothing chosen: Remove is disabled")
        page.click("#dlg label:has-text('Also remove') input")
        check(page.is_enabled("#stale-remove"), "ticked: Remove is enabled")
        page.click("#dlg button:has-text('Cancel')")
        check(removed() == [], "Cancel removes nothing")
        # None at all: said so, with OK.
        stale_dialog("running", [])
        buttons = page.eval_on_selector_all("#dlg button", "bs => bs.map(b => b.textContent)")
        check("No worker containers found." in text("#dlg") and buttons == ["OK"], "no worker containers: said so, OK")
        page.click("#dlg button:has-text('OK')")

        check(not errors, f"no script errors {errors}")
        browser.close()
    server.shutdown()
    print(f"{len(failures)} failed" if failures else "every click did what it should")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())

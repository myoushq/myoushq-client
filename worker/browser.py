#!/usr/bin/env python3
"""Keep a visible Chromium running for the worker's scripts to attach to.

Scripts the worker runs (sent by your agent with `myous exec`) connect to
it with Playwright:

    from playwright.sync_api import sync_playwright
    with sync_playwright() as p:
        browser = p.chromium.connect_over_cdp("http://localhost:9222")
        page = browser.contexts[0].new_page()

The browser is started as a plain process, not through Playwright: a
browser launched by Playwright carries the automation mark
(navigator.webdriver is true on every page) for as long as Playwright is
attached, and sites that screen for bots refuse it even while the owner
drives it by hand through the browser view. Started plainly, nothing is
attached until an agent's script connects, and that connection does not
set the mark.

The browser uses a persistent profile, so sites you log into once stay
logged in. In the container the window shows on the display that noVNC
serves (http://localhost:6080/); in direct mode it's an ordinary window
on your screen. If the browser exits (you closed it, it crashed), it's
started again a few seconds later.

Environment: MYOUS_BROWSER_PROFILE (profile directory, default
~/.myous-worker/profile), MYOUS_CDP_PORT (default 9222),
MYOUS_BROWSER_NO_SANDBOX=1 (needed inside the container, where Chromium's
own sandbox can't be set up), MYOUS_BROWSER_BIN (the browser to run;
default: Playwright's Chromium, else chromium or google-chrome on PATH),
MYOUS_LANG (the browser's language, e.g. en-US; the container gets the
owner's from the app).
"""
from __future__ import annotations

import os
import shutil
import subprocess
import sys
import time

PROFILE = os.path.expanduser(os.environ.get("MYOUS_BROWSER_PROFILE", "~/.myous-worker/profile"))
PORT = int(os.environ.get("MYOUS_CDP_PORT", "9222"))
START_PAGE = "about:blank"


def log(msg: str) -> None:
    print(f"browser: {msg}", file=sys.stderr, flush=True)


def clear_stale_lock() -> None:
    """Chromium leaves SingletonLock (a symlink to "<hostname>-<pid>") in the
    profile when it's killed rather than closed, for instance when the
    container is stopped hard. A new container has a new hostname, so
    Chromium then thinks the profile is in use on another computer and
    refuses to start. Only one browser ever runs on this profile, so at
    start the lock is stale by definition."""
    for name in ("SingletonLock", "SingletonSocket", "SingletonCookie"):
        path = os.path.join(PROFILE, name)
        if os.path.islink(path) or os.path.exists(path):
            try:
                os.unlink(path)
                log(f"removed stale {name}")
            except OSError as e:
                log(f"could not remove {name}: {e}")


def executable() -> str | None:
    """The browser binary: MYOUS_BROWSER_BIN, Playwright's Chromium (what the
    image ships), or a chromium / google-chrome on PATH."""
    env = os.environ.get("MYOUS_BROWSER_BIN")
    if env:
        return env
    try:
        from playwright.sync_api import sync_playwright

        with sync_playwright() as p:
            path = p.chromium.executable_path
        if path and os.path.exists(path):
            return path
    except Exception as e:  # no Playwright, or its browsers aren't installed
        log(f"no Playwright Chromium ({type(e).__name__}); looking on PATH")
    for name in ("chromium", "chromium-browser", "google-chrome", "google-chrome-stable"):
        found = shutil.which(name)
        if found:
            return found
    return None


def arguments(exe: str) -> list[str]:
    args = [
        exe,
        f"--remote-debugging-port={PORT}",
        "--remote-debugging-address=127.0.0.1",
        f"--user-data-dir={PROFILE}",
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-search-engine-choice-screen",
        "--start-maximized",
    ]
    lang = os.environ.get("MYOUS_LANG")
    if lang:
        args.append(f"--lang={lang}")
    if sys.platform == "darwin":
        args.append("--use-mock-keychain")   # never a login-keychain dialog
    else:
        args.append("--password-store=basic")
    if os.environ.get("MYOUS_BROWSER_NO_SANDBOX") == "1":
        args.append("--no-sandbox")
    args.append(START_PAGE)
    return args


def run_once(exe: str) -> int:
    clear_stale_lock()
    proc = subprocess.Popen(arguments(exe))
    log(f"running {exe} (pid {proc.pid}), profile {PROFILE}, CDP on 127.0.0.1:{PORT}")
    return proc.wait()


def main() -> None:
    os.makedirs(PROFILE, mode=0o700, exist_ok=True)
    exe = executable()
    if not exe:
        log("no browser found; set MYOUS_BROWSER_BIN")
        sys.exit(1)
    while True:
        try:
            status = run_once(exe)
            log(f"exited with status {status}; restarting in 3 s")
        except KeyboardInterrupt:
            return
        except Exception as e:  # display not ready yet, binary gone
            log(f"could not run ({type(e).__name__}: {str(e).splitlines()[0] if str(e) else ''}); retrying in 3 s")
        time.sleep(3)


if __name__ == "__main__":
    main()

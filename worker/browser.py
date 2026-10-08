#!/usr/bin/env python3
"""Keep a visible Chromium running for the worker's scripts to attach to.

Scripts the worker runs (sent by your agent with `myous exec`) connect to
it with Playwright:

    from playwright.sync_api import sync_playwright
    with sync_playwright() as p:
        browser = p.chromium.connect_over_cdp("http://localhost:9222")
        page = browser.contexts[0].new_page()

The browser uses a persistent profile, so sites you log into once stay
logged in. In the container the window shows on the Xvfb display that
noVNC serves (http://localhost:6080/vnc.html); in direct mode it's an
ordinary window on your screen. If the browser exits (you closed it, it
crashed), it's started again a few seconds later.

Environment: MYOUS_BROWSER_PROFILE (profile directory, default
~/.myous-worker/profile), MYOUS_CDP_PORT (default 9222),
MYOUS_BROWSER_NO_SANDBOX=1 (needed inside the container, where Chromium's
own sandbox can't be set up).
"""
from __future__ import annotations

import os
import sys
import time

from playwright.sync_api import sync_playwright

PROFILE = os.path.expanduser(os.environ.get("MYOUS_BROWSER_PROFILE", "~/.myous-worker/profile"))
PORT = int(os.environ.get("MYOUS_CDP_PORT", "9222"))
START_PAGE = "about:blank"


def log(msg: str) -> None:
    print(f"browser: {msg}", file=sys.stderr, flush=True)


def run_once() -> None:
    args = [f"--remote-debugging-port={PORT}", "--remote-debugging-address=127.0.0.1"]
    if os.environ.get("MYOUS_BROWSER_NO_SANDBOX") == "1":
        args.append("--no-sandbox")
    with sync_playwright() as p:
        context = p.chromium.launch_persistent_context(
            PROFILE, headless=False, args=args, viewport=None, no_viewport=True,
        )
        page = context.pages[0] if context.pages else context.new_page()
        page.goto(START_PAGE)
        log(f"running, profile {PROFILE}, CDP on 127.0.0.1:{PORT}")
        # The sync API only delivers events while we're inside a call, so
        # keep calling into it; a closed context makes the call raise.
        while True:
            page = next(iter(context.pages), None)
            if page is None:
                page = context.new_page()
            page.wait_for_timeout(1000)


def main() -> None:
    os.makedirs(PROFILE, mode=0o700, exist_ok=True)
    while True:
        try:
            run_once()
        except KeyboardInterrupt:
            return
        except Exception as e:  # closed window, crash, display not ready yet
            log(f"stopped ({type(e).__name__}: {str(e).splitlines()[0] if str(e) else ''}); restarting in 3 s")
        time.sleep(3)


if __name__ == "__main__":
    main()

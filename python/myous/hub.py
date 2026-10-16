"""The hub's HTTPS API: its config and the pairing mailbox."""
from __future__ import annotations

import http.client
import json
import time
import urllib.error
import urllib.request
from typing import Any

from myous.storage import Storage

DEFAULT_HUB = "https://myoushq.com"
CONFIG_MAX_AGE = 6 * 3600
# After a refresh fails, the cached config serves this long before the next
# try: each try may wait for its timeout, and a poll asks more than once.
CONFIG_RETRY = 600
CONFIG_TIMEOUT = 10


class HubError(Exception):
    def __init__(self, status: int, message: str):
        super().__init__(f"hub error {status}: {message}")
        self.status = status
        self.message = message


class Hub:
    def __init__(self, storage: Storage, url: str | None = None):
        self.storage = storage
        self.url = (url or storage.get("settings", {}).get("hub") or DEFAULT_HUB).rstrip("/")

    def config(self, refresh: bool = False) -> dict:
        """Relay list and other settings, cached and refreshed every few hours."""
        cached = self.storage.get("hub")
        now = time.time()
        fresh = cached and cached.get("url") == self.url and (
            now - cached.get("fetched_at", 0) < CONFIG_MAX_AGE or now < cached.get("retry_at", 0))
        if fresh and not refresh:
            return cached
        try:
            cfg = self.request("GET", "/config.json", timeout=CONFIG_TIMEOUT)
        except (HubError, OSError):
            if cached and cached.get("url") == self.url:
                # Hub unreachable: keep using what we had, and don't ask again for a while.
                self.storage.put("hub", dict(cached, retry_at=int(now) + CONFIG_RETRY))
                return cached
            raise
        cfg.update(url=self.url, fetched_at=int(time.time()))
        self.storage.put("hub", cfg)
        return cfg

    def request(self, method: str, endpoint: str, body: Any = None, token: str | None = None,
                timeout: float = 30) -> Any:
        req = urllib.request.Request(self.url + endpoint, method=method)
        req.add_header("Accept", "application/json")
        data = None
        if body is not None:
            data = json.dumps(body).encode()
            req.add_header("Content-Type", "application/json")
        if token:
            req.add_header("Authorization", "Bearer " + token)
        try:
            with urllib.request.urlopen(req, data=data, timeout=timeout) as resp:
                raw = resp.read()
        except urllib.error.HTTPError as e:
            try:
                message = json.loads(e.read()).get("error", e.reason)
            except ValueError:
                message = e.reason
            raise HubError(e.code, message) from None
        except http.client.HTTPException as e:
            # A dropped connection mid-response, e.g. a proxy cutting a long poll.
            raise ConnectionError(f"lost the connection to {self.url}: {e!r}") from None
        return json.loads(raw) if raw else None

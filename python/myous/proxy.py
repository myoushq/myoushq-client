"""Outbound proxy support, for agents whose network only lets them out
through an HTTP proxy (common in sandboxes and offices).

The standard variables choose the proxy: HTTPS_PROXY for wss:// and
https://, HTTP_PROXY for ws:// and http://, ALL_PROXY as a fallback, and
NO_PROXY for exceptions (lowercase names work too). Hub requests use them
through urllib. The relay connection goes through nostr-sdk, which only
speaks SOCKS5, so for an http:// proxy we run a small SOCKS5 server on
127.0.0.1 that turns each connection into an HTTP CONNECT through the proxy.
TLS to the relay stays end to end: the proxy sees the relay's host name,
not the traffic.
"""
from __future__ import annotations

import base64
import select
import socket
import ssl
import struct
import threading
import urllib.parse
import urllib.request

_bridges: dict[str, "Socks5Bridge"] = {}
_lock = threading.Lock()


def proxy_for(url: str) -> str | None:
    """The proxy URL to use for `url`, from the environment, or None."""
    u = urllib.parse.urlparse(url)
    if not u.hostname or urllib.request.proxy_bypass_environment(u.hostname):
        return None
    env = urllib.request.getproxies_environment()
    scheme = "https" if u.scheme in ("https", "wss") else "http"
    return env.get(scheme) or env.get("all") or None


def relay_proxy(relay_urls: list[str]) -> str | None:
    """A SOCKS5 address ("host:port") for nostr-sdk to reach the relays
    through, or None to connect directly."""
    proxy = next((p for p in map(proxy_for, relay_urls) if p), None)
    if proxy is None:
        return None
    if "://" not in proxy:
        proxy = "http://" + proxy
    p = urllib.parse.urlparse(proxy)
    if p.scheme in ("socks5", "socks5h"):
        if p.username:
            raise ValueError("SOCKS5 proxies with a password aren't supported; use an http:// proxy")
        return f"{socket.gethostbyname(p.hostname)}:{p.port or 1080}"
    if p.scheme not in ("http", "https"):
        raise ValueError(f"unsupported proxy {p.scheme}://; use http://, https:// or socks5://")
    with _lock:
        if proxy not in _bridges:
            _bridges[proxy] = Socks5Bridge(p)
        return _bridges[proxy].address


class Socks5Bridge:
    """SOCKS5 (no auth) on 127.0.0.1, forwarding through an HTTP proxy with
    CONNECT. Runs in daemon threads for the life of the process."""

    def __init__(self, proxy: urllib.parse.ParseResult):
        self.proxy = proxy
        self.server = socket.create_server(("127.0.0.1", 0))
        self.address = "127.0.0.1:%d" % self.server.getsockname()[1]
        threading.Thread(target=self._serve, daemon=True).start()

    def _serve(self) -> None:
        while True:
            client, _ = self.server.accept()
            threading.Thread(target=self._handle, args=(client,), daemon=True).start()

    def _handle(self, client: socket.socket) -> None:
        upstream = None
        try:
            host, port = _socks_request(client)
            try:
                upstream, early = self._connect(host, port)
            except OSError:
                client.sendall(b"\x05\x05\x00\x01" + bytes(6))  # connection refused
                return
            client.sendall(b"\x05\x00\x00\x01" + bytes(6) + early)
            _pipe(client, upstream)
        except (OSError, ValueError):
            pass
        finally:
            client.close()
            if upstream:
                upstream.close()

    def _connect(self, host: str, port: int) -> tuple[socket.socket, bytes]:
        """Open a tunnel to host:port through the proxy. Returns the socket
        and any bytes that arrived after the proxy's reply."""
        p = self.proxy
        sock = socket.create_connection((p.hostname, p.port or (443 if p.scheme == "https" else 80)), timeout=30)
        if p.scheme == "https":
            sock = ssl.create_default_context().wrap_socket(sock, server_hostname=p.hostname)
        target = f"[{host}]:{port}" if ":" in host else f"{host}:{port}"
        request = f"CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n"
        if p.username:
            creds = f"{urllib.parse.unquote(p.username)}:{urllib.parse.unquote(p.password or '')}"
            request += f"Proxy-Authorization: Basic {base64.b64encode(creds.encode()).decode()}\r\n"
        sock.sendall((request + "\r\n").encode())
        head = b""
        while b"\r\n\r\n" not in head:
            chunk = sock.recv(4096)
            if not chunk or len(head) > 65536:
                raise OSError("proxy closed the connection")
            head += chunk
        head, early = head.split(b"\r\n\r\n", 1)
        status_line = head.split(b"\r\n", 1)[0]
        status = status_line.split()
        if len(status) < 2 or status[1] != b"200":
            raise OSError("proxy refused CONNECT: " + status_line.decode(errors="replace"))
        sock.settimeout(None)
        return sock, early


def _recv_exact(sock: socket.socket, n: int) -> bytes:
    data = b""
    while len(data) < n:
        chunk = sock.recv(n - len(data))
        if not chunk:
            raise ValueError("connection closed")
        data += chunk
    return data


def _socks_request(client: socket.socket) -> tuple[str, int]:
    """Read a SOCKS5 greeting and CONNECT request; return the target."""
    version, n_methods = _recv_exact(client, 2)
    if version != 5 or 0 not in _recv_exact(client, n_methods):
        raise ValueError("not SOCKS5 without auth")
    client.sendall(b"\x05\x00")
    version, command, _, atyp = _recv_exact(client, 4)
    if version != 5 or command != 1:
        client.sendall(b"\x05\x07\x00\x01" + bytes(6))  # only CONNECT
        raise ValueError("unsupported SOCKS command")
    if atyp == 1:
        host = socket.inet_ntoa(_recv_exact(client, 4))
    elif atyp == 3:
        host = _recv_exact(client, _recv_exact(client, 1)[0]).decode()
    elif atyp == 4:
        host = socket.inet_ntop(socket.AF_INET6, _recv_exact(client, 16))
    else:
        raise ValueError("bad address type")
    (port,) = struct.unpack(">H", _recv_exact(client, 2))
    return host, port


def _pipe(a: socket.socket, b: socket.socket) -> None:
    socks = [a, b]
    while True:
        readable, _, _ = select.select(socks, [], [], 300)
        if not readable:
            return  # idle for 5 minutes; the client pings more often than that
        for s in readable:
            data = s.recv(65536) if not isinstance(s, ssl.SSLSocket) else _recv_ssl(s)
            if not data:
                return
            (b if s is a else a).sendall(data)


def _recv_ssl(s: ssl.SSLSocket) -> bytes:
    # Drain what the TLS layer has already decrypted, or select() won't wake for it.
    data = s.recv(65536)
    while data and s.pending():
        data += s.recv(s.pending())
    return data

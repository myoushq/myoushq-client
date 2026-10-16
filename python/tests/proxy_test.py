"""The SOCKS5 bridge (proxy.py) listens on 127.0.0.1 without
authentication and adds the user's proxy credentials to each CONNECT, for
as long as the process runs (a listener: for days). Any local process or
user can connect to it, so it must only open tunnels to the relays it was
made for."""
import os
import socket
import struct
import threading
import unittest
from unittest import mock

from myous import proxy


class FakeProxy:
    """An HTTP proxy that records each CONNECT line and says yes."""

    def __init__(self):
        self.seen = []
        self.server = socket.create_server(("127.0.0.1", 0))
        threading.Thread(target=self.serve, daemon=True).start()

    def serve(self):
        while True:
            try:
                c, _ = self.server.accept()
            except OSError:
                return
            self.seen.append(c.recv(4096).decode().split("\r\n")[0])
            c.sendall(b"HTTP/1.1 200 Connection established\r\n\r\n")
            c.close()


def socks_connect(address: str, host: str, port: int) -> int:
    """The SOCKS5 reply code for CONNECT host:port (0 = succeeded)."""
    h, p = address.split(":")
    with socket.create_connection((h, int(p)), timeout=5) as s:
        s.sendall(b"\x05\x01\x00")
        s.recv(2)
        s.sendall(b"\x05\x01\x00\x03" + bytes([len(host)]) + host.encode() + struct.pack(">H", port))
        return s.recv(10)[1]


class Bridge(unittest.TestCase):
    def setUp(self):
        self.proxy = FakeProxy()
        self.addCleanup(self.proxy.server.close)
        env = {k: v for k, v in os.environ.items() if k.lower() not in ("no_proxy", "all_proxy")}
        env["HTTPS_PROXY"] = f"http://alice:s3cret@127.0.0.1:{self.proxy.server.getsockname()[1]}"
        patcher = mock.patch.dict(os.environ, env, clear=True)
        patcher.start()
        self.addCleanup(patcher.stop)
        self.addCleanup(proxy._bridges.clear)

    def test_tunnels_only_to_the_relays(self):
        address = proxy.relay_proxy(["wss://Relay.Example.com", "wss://relay2.example.com:7777"])
        self.assertEqual(socks_connect(address, "relay.example.com", 443), 0)
        self.assertEqual(socks_connect(address, "relay2.example.com", 7777), 0)
        self.assertNotEqual(socks_connect(address, "internal.example.net", 22), 0)
        self.assertNotEqual(socks_connect(address, "relay.example.com", 22), 0)
        self.assertEqual(self.proxy.seen, ["CONNECT relay.example.com:443 HTTP/1.1",
                                           "CONNECT relay2.example.com:7777 HTTP/1.1"])

    def test_a_later_relay_list_adds_to_the_same_bridge(self):
        first = proxy.relay_proxy(["wss://relay.example.com"])
        self.assertEqual(proxy.relay_proxy(["wss://other.example.com"]), first)
        self.assertEqual(socks_connect(first, "other.example.com", 443), 0)


if __name__ == "__main__":
    unittest.main()

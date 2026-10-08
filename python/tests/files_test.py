"""File encryption, name hygiene and the blob authorization header."""
import base64
import json
import unittest

from nostr_sdk import Event, Keys

from myous import files


class Encryption(unittest.TestCase):
    def test_round_trip_and_hashes(self):
        enc = files.encrypt(b"hello")
        self.assertEqual(len(enc.key), 32)
        self.assertEqual(len(enc.nonce), 12)
        self.assertEqual(enc.ox, files.sha256(b"hello"))
        self.assertEqual(enc.x, files.sha256(enc.ciphertext))
        self.assertEqual(files.decrypt(enc.ciphertext, enc.key, enc.nonce, enc.x, enc.ox), b"hello")

    def test_tampering_is_detected(self):
        enc = files.encrypt(b"hello")
        altered = bytes([enc.ciphertext[0] ^ 1]) + enc.ciphertext[1:]
        with self.assertRaises(ValueError):
            files.decrypt(altered, enc.key, enc.nonce, enc.x, enc.ox)  # hash mismatch
        with self.assertRaises(ValueError):
            files.decrypt(altered, enc.key, enc.nonce)  # GCM tag mismatch
        with self.assertRaises(ValueError):
            files.decrypt(enc.ciphertext, enc.key, enc.nonce, enc.x, "00" * 32)  # wrong plaintext hash

    def test_size_cap(self):
        # The ciphertext is the plaintext plus the tag, and the hub caps the ciphertext.
        self.assertEqual(files.MAX_PLAINTEXT + files.TAG_BYTES, files.MAX_BLOB)
        with self.assertRaises(ValueError):
            files.encrypt(b"x" * (files.MAX_PLAINTEXT + 1))

    def test_downloads_are_bounded(self):
        import http.server
        import threading

        class Big(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                body = b"x" * (150 if self.path.endswith("/" + "a" * 64) else 50)
                self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *a):
                pass

        srv = http.server.HTTPServer(("127.0.0.1", 0), Big)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        try:
            client = files.BlobClient(Keys.generate(), f"http://127.0.0.1:{srv.server_port}/blob")
            client.max_blob = 100
            with self.assertRaises(ValueError):
                client.get("a" * 64)  # more than the cap: refused before it's all read
            self.assertEqual(len(client.get("b" * 64)), 50)
            with self.assertRaises(ValueError):
                client.get("b" * 64, size=51)  # the hub announces a size the message didn't
        finally:
            srv.shutdown()
            srv.server_close()

    def test_safe_name(self):
        self.assertEqual(files.safe_name("report.pdf"), "report.pdf")
        self.assertEqual(files.safe_name("../../etc/passwd"), "passwd")
        self.assertEqual(files.safe_name("C:\\Users\\x\\a.txt"), "a.txt")
        for bad in ("", ".", "..", "dir/", "x" * 256, None, 3):
            self.assertIsNone(files.safe_name(bad), bad)
        self.assertEqual(files.safe_name("a\x00b"), "ab")


class FileTags(unittest.TestCase):
    def test_tags_round_trip(self):
        enc = files.encrypt(b"data")
        tags = files.file_tags(enc, "a.txt", "text/plain", [["w", "put", "ab" * 16, "in/a.txt"]])
        blob_api = "https://myoushq.com/blob"
        parsed = files.parse_file_tags(tags, f"{blob_api}/{enc.x}", blob_api)
        self.assertEqual(parsed["name"], "a.txt")
        self.assertEqual(parsed["mime"], "text/plain")
        self.assertEqual(parsed["size"], len(enc.ciphertext))
        self.assertEqual((parsed["key"], parsed["nonce"]), (enc.key.hex(), enc.nonce.hex()))
        self.assertEqual(parsed["w"], ["put", "ab" * 16, "in/a.txt"])
        # Only our own hub's store is accepted, and the URL must name the blob.
        self.assertIsNone(files.parse_file_tags(tags, f"https://evil.example/{enc.x}", blob_api))
        self.assertIsNone(files.parse_file_tags(tags, f"{blob_api}/{'0' * 64}", blob_api))
        self.assertIsNone(files.parse_file_tags(tags, f"{blob_api}/{enc.x}", None))
        # A bad name or a bad key is dropped, not sanitized into something surprising.
        bad = [t if t[0] != "name" else ["name", ".."] for t in tags]
        self.assertIsNone(files.parse_file_tags(bad, f"{blob_api}/{enc.x}", blob_api))
        bad = [t if t[0] != "decryption-key" else ["decryption-key", "zz"] for t in tags]
        self.assertIsNone(files.parse_file_tags(bad, f"{blob_api}/{enc.x}", blob_api))


class Authorization(unittest.TestCase):
    def test_header_is_a_signed_kind_24242_event(self):
        keys = Keys.generate()
        client = files.BlobClient(keys, "https://myoushq.com/blob/")
        header = client.authorization("upload", "ab" * 32, now=1_800_000_000)
        scheme, token = header.split(" ")
        self.assertEqual(scheme, "Nostr")
        self.assertNotIn("=", token)
        raw = base64.urlsafe_b64decode(token + "=" * (-len(token) % 4))
        obj = json.loads(raw)
        self.assertEqual(obj["kind"], 24242)
        self.assertEqual(obj["pubkey"], keys.public_key().to_hex())
        tags = {t[0]: t[1] for t in obj["tags"]}
        self.assertEqual(tags["t"], "upload")
        self.assertEqual(tags["x"], "ab" * 32)
        self.assertEqual(int(tags["expiration"]), 1_800_000_000 + files.AUTH_TTL)
        self.assertTrue(Event.from_json(raw.decode()).verify())
        self.assertEqual(client.url, "https://myoushq.com/blob")

    def test_no_blob_api(self):
        with self.assertRaises(ValueError):
            files.BlobClient(Keys.generate(), "")


class UniquePath(unittest.TestCase):
    def test_numbering(self):
        import tempfile
        from pathlib import Path
        with tempfile.TemporaryDirectory() as d:
            self.assertEqual(files.unique_path(d, "a.txt"), str(Path(d) / "a.txt"))
            (Path(d) / "a.txt").write_text("x")
            self.assertEqual(files.unique_path(d, "a.txt"), str(Path(d) / "a (2).txt"))
            (Path(d) / "a (2).txt").write_text("x")
            self.assertEqual(files.unique_path(d, "a.txt"), str(Path(d) / "a (3).txt"))


if __name__ == "__main__":
    unittest.main()

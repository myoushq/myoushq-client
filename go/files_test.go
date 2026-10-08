package myous

import (
	"bytes"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/nbd-wtf/go-nostr"
)

func TestFileRoundTripAndTamper(t *testing.T) {
	plain := []byte("the quick brown fox")
	ef, err := EncryptFile(plain)
	if err != nil {
		t.Fatal(err)
	}
	if len(ef.Ciphertext) != len(plain)+16 || ef.X != sha256Hex(ef.Ciphertext) || ef.Ox != sha256Hex(plain) {
		t.Fatalf("unexpected encryption output: %+v", ef)
	}
	got, err := DecryptFile(ef.Ciphertext, ef.Key, ef.Nonce, ef.X, ef.Ox)
	if err != nil || !bytes.Equal(got, plain) {
		t.Fatalf("round trip: %v %q", err, got)
	}

	flipped := append([]byte(nil), ef.Ciphertext...)
	flipped[3] ^= 1
	if _, err := DecryptFile(flipped, ef.Key, ef.Nonce, ef.X, ef.Ox); err == nil || !strings.Contains(err.Error(), "(x)") {
		t.Fatalf("tampered blob accepted: %v", err)
	}
	if _, err := DecryptFile(flipped, ef.Key, ef.Nonce, sha256Hex(flipped), ef.Ox); err == nil || !strings.Contains(err.Error(), "decrypt") {
		t.Fatalf("tampered blob with matching x accepted: %v", err)
	}
	if _, err := DecryptFile(ef.Ciphertext, ef.Key, ef.Nonce, ef.X, sha256Hex([]byte("other"))); err == nil || !strings.Contains(err.Error(), "(ox)") {
		t.Fatalf("wrong ox accepted: %v", err)
	}
	if _, err := DecryptFile(ef.Ciphertext, ef.Key[:31], ef.Nonce, ef.X, ef.Ox); err == nil {
		t.Fatal("short key accepted")
	}
}

// Checks against the shared vectors in docs/test-vectors.json, when the
// "files" key exists (the Python client writes it).
func TestFileVectors(t *testing.T) {
	raw, err := os.ReadFile(filepath.Join("..", "docs", "test-vectors.json"))
	if err != nil {
		t.Skip("no test vectors file")
	}
	var doc struct {
		Files []struct {
			Key, Nonce, PlaintextHex, CiphertextHex, X, Ox string
		} `json:"files"`
	}
	if err := json.Unmarshal(bytes.ReplaceAll(bytes.ReplaceAll(raw, []byte(`"plaintext_hex"`), []byte(`"PlaintextHex"`)), []byte(`"ciphertext_hex"`), []byte(`"CiphertextHex"`)), &doc); err != nil {
		t.Fatal(err)
	}
	if len(doc.Files) == 0 {
		t.Skip("test-vectors.json has no \"files\" vectors yet")
	}
	for i, v := range doc.Files {
		key, _ := hex.DecodeString(v.Key)
		nonce, _ := hex.DecodeString(v.Nonce)
		plain, _ := hex.DecodeString(v.PlaintextHex)
		ef, err := encryptFileWith(key, nonce, plain)
		if err != nil {
			t.Fatal(err)
		}
		if hex.EncodeToString(ef.Ciphertext) != v.CiphertextHex || ef.X != v.X || ef.Ox != v.Ox {
			t.Fatalf("vector %d: got %s x=%s ox=%s", i, hex.EncodeToString(ef.Ciphertext), ef.X, ef.Ox)
		}
		ct, _ := hex.DecodeString(v.CiphertextHex)
		if got, err := DecryptFile(ct, key, nonce, v.X, v.Ox); err != nil || !bytes.Equal(got, plain) {
			t.Fatalf("vector %d decrypt: %v", i, err)
		}
	}
}

func TestSanitizeName(t *testing.T) {
	cases := map[string]string{
		"report.pdf": "report.pdf", "../../key": "key", "/etc/passwd": "passwd", "dir\\evil.exe": "evil.exe",
		"a\x00b.txt": "ab.txt", "trailing/": "trailing", "": "", ".": "", "..": "", "/": "",
	}
	for in, want := range cases {
		got, ok := sanitizeName(in)
		if got != want || ok != (want != "") {
			t.Errorf("sanitizeName(%q) = %q, %v; want %q", in, got, ok, want)
		}
	}
}

func TestBlobAuthHeader(t *testing.T) {
	sk := nostr.GeneratePrivateKey()
	pk, _ := nostr.GetPublicKey(sk)
	c := NewBlobClient(sk, "http://hub.test/blob/")
	if c.API != "http://hub.test/blob" {
		t.Fatalf("API not trimmed: %q", c.API)
	}
	x := strings.Repeat("ab", 32)
	header, err := c.authHeader("upload", x)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.HasPrefix(header, "Nostr ") || strings.ContainsAny(header, "=+/") {
		t.Fatalf("header isn't base64url without padding: %q", header)
	}
	raw, err := base64.RawURLEncoding.DecodeString(strings.TrimPrefix(header, "Nostr "))
	if err != nil {
		t.Fatal(err)
	}
	var evt nostr.Event
	if err := json.Unmarshal(raw, &evt); err != nil {
		t.Fatal(err)
	}
	if ok, _ := evt.CheckSignature(); !ok || evt.PubKey != pk || evt.Kind != kindBlobAuth {
		t.Fatalf("bad event: %+v", evt)
	}
	if evt.Tags.GetFirst([]string{"t", ""}).Value() != "upload" || evt.Tags.GetFirst([]string{"x", ""}).Value() != x {
		t.Fatalf("tags: %v", evt.Tags)
	}
	exp := evt.Tags.GetFirst([]string{"expiration", ""}).Value()
	if exp == "" {
		t.Fatal("no expiration tag")
	}
	var expAt int64
	if err := json.Unmarshal([]byte(exp), &expAt); err != nil {
		t.Fatal(err)
	}
	now := time.Now().Unix()
	if expAt <= now || expAt > now+600 || int64(evt.CreatedAt) > now || int64(evt.CreatedAt) < now-5 {
		t.Fatalf("times: created %d, expires %d, now %d", evt.CreatedAt, expAt, now)
	}
}

func TestParseFileTagsAndBlobURL(t *testing.T) {
	ef, _ := EncryptFile([]byte("hi"))
	tags := fileTags(ef, "notes.txt", "text/plain", nostr.Tags{{"w", "file", "abc"}})
	f, ok := parseFileTags(tags, "http://hub.test/blob/"+ef.X)
	if !ok || f.Name != "notes.txt" || f.Mime != "text/plain" || f.Size != int64(len(ef.Ciphertext)) ||
		!bytes.Equal(f.Key, ef.Key) || f.X != ef.X || f.Ox != ef.Ox || strings.Join(f.W, ",") != "file,abc" {
		t.Fatalf("parsed: %+v (%v)", f, ok)
	}
	bad := fileTags(ef, "../x", "text/plain", nil)
	if f, ok := parseFileTags(bad, ""); !ok || f.Name != "x" {
		t.Fatalf("name not sanitized: %+v", f)
	}
	for _, broken := range []nostr.Tags{
		fileTags(ef, "", "", nil),
		append(nostr.Tags{{"encryption-algorithm", "rot13"}}, fileTags(ef, "a", "", nil)[1:]...),
		append(nostr.Tags{{"decryption-key", "zz"}}, fileTags(ef, "a", "", nil)...),
	} {
		if _, ok := parseFileTags(broken, ""); ok {
			t.Fatalf("malformed tags accepted: %v", broken)
		}
	}
	if x, ok := blobURLHash("http://hub.test/blob/"+ef.X, "http://hub.test/blob"); !ok || x != ef.X {
		t.Fatal("own hub URL refused")
	}
	for _, u := range []string{"http://evil.test/blob/" + ef.X, "http://hub.test/blob/../" + ef.X, "http://hub.test/blob/nothex", ""} {
		if _, ok := blobURLHash(u, "http://hub.test/blob"); ok {
			t.Fatalf("foreign URL accepted: %s", u)
		}
	}
	if _, ok := blobURLHash("http://hub.test/blob/"+ef.X, ""); ok {
		t.Fatal("accepted without a known blob_api")
	}
}

func TestWorkerRepliesAreNotUnread(t *testing.T) {
	kind, id, ok := workerReply(`{"myous":"result","id":"abc","exit":0}`)
	if !ok || kind != "result" || id != "abc" {
		t.Fatalf("reply not recognized: %s %s %v", kind, id, ok)
	}
	if _, _, ok := workerReply(`{"myous":"exec","id":"abc","cmd":"ls"}`); ok {
		t.Fatal("a request was taken for a reply")
	}
	if _, _, ok := workerReply("hello {"); ok {
		t.Fatal("plain text taken for a reply")
	}
	for _, e := range []Entry{{Type: "result"}, {Type: "ack"}, {Type: "file", W: []string{"file", "id"}}} {
		if !e.consumedByCommand() {
			t.Fatalf("%+v should be consumed by a command", e)
		}
	}
	for _, e := range []Entry{{Type: "message"}, {Type: "file"}, {Type: "file", W: []string{"put", "id", "p"}}} {
		if e.consumedByCommand() {
			t.Fatalf("%+v should be unread", e)
		}
	}
}

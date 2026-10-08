package myous

import (
	"bytes"
	"context"
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"path"
	"strconv"
	"strings"
	"time"

	"github.com/nbd-wtf/go-nostr"
)

// Files (protocol.md section 6): a file is encrypted with a fresh key,
// the ciphertext is stored on the hub as a blob named by its hash, and the
// key travels inside an end-to-end encrypted file message (kind 15). The
// hub holds bytes it can't read and learns neither recipient nor name.

const (
	kindBlobAuth = 24242
	blobAuthTTL  = 5 * time.Minute
	fileKeyLen   = 32
	fileNonceLen = 12
)

// EncryptedFile is a file ready to upload: the ciphertext and what the
// recipient needs to open it.
type EncryptedFile struct {
	Key, Nonce []byte
	Ciphertext []byte
	X, Ox      string // sha256 hex of the ciphertext and of the plaintext
}

// EncryptFile encrypts plaintext with AES-256-GCM under a random key and
// nonce (the 16-byte tag is appended, as the library does).
func EncryptFile(plaintext []byte) (*EncryptedFile, error) {
	key, nonce := make([]byte, fileKeyLen), make([]byte, fileNonceLen)
	if _, err := rand.Read(key); err != nil {
		return nil, err
	}
	if _, err := rand.Read(nonce); err != nil {
		return nil, err
	}
	return encryptFileWith(key, nonce, plaintext)
}

// encryptFileWith is EncryptFile with a chosen key and nonce (test vectors).
func encryptFileWith(key, nonce, plaintext []byte) (*EncryptedFile, error) {
	gcm, err := fileCipher(key, nonce)
	if err != nil {
		return nil, err
	}
	ct := gcm.Seal(nil, nonce, plaintext, nil)
	return &EncryptedFile{Key: key, Nonce: nonce, Ciphertext: ct, X: sha256Hex(ct), Ox: sha256Hex(plaintext)}, nil
}

// DecryptFile checks that the ciphertext has hash x, decrypts it, and checks
// that the result has hash ox. Both checks catch a wrong or tampered blob
// before anything is written.
func DecryptFile(ciphertext, key, nonce []byte, x, ox string) ([]byte, error) {
	if sha256Hex(ciphertext) != x {
		return nil, errors.New("the blob's hash doesn't match the message (x)")
	}
	gcm, err := fileCipher(key, nonce)
	if err != nil {
		return nil, err
	}
	plain, err := gcm.Open(nil, nonce, ciphertext, nil)
	if err != nil {
		return nil, errors.New("the blob doesn't decrypt with the message's key")
	}
	if sha256Hex(plain) != ox {
		return nil, errors.New("the decrypted file's hash doesn't match the message (ox)")
	}
	return plain, nil
}

func fileCipher(key, nonce []byte) (cipher.AEAD, error) {
	if len(key) != fileKeyLen || len(nonce) != fileNonceLen {
		return nil, fmt.Errorf("file key must be %d bytes and nonce %d bytes", fileKeyLen, fileNonceLen)
	}
	block, err := aes.NewCipher(key)
	if err != nil {
		return nil, err
	}
	return cipher.NewGCM(block)
}

func sha256Hex(b []byte) string {
	sum := sha256.Sum256(b)
	return hex.EncodeToString(sum[:])
}

// sanitizeName keeps only the last path component of a file name from a
// message and reports whether anything usable is left. Senders are
// contacts, not trusted software: "../key" must not escape the directory.
func sanitizeName(name string) (string, bool) {
	name = strings.ReplaceAll(name, "\\", "/")
	name = path.Base(strings.TrimRight(name, "/"))
	name = strings.Map(func(r rune) rune {
		if r < 0x20 || r == 0x7f {
			return -1
		}
		return r
	}, name)
	if name == "" || name == "." || name == ".." || name == "/" {
		return "", false
	}
	return name, true
}

// fileInfo is what a kind-15 rumor carries.
type fileInfo struct {
	Name, Mime, URL string
	Key, Nonce      []byte
	X, Ox           string
	Size            int64
	W               []string // the ["w", ...] tag's values, for workers
}

// parseFileTags reads the tags of a kind-15 rumor; malformed ones are dropped.
func parseFileTags(tags nostr.Tags, content string) (*fileInfo, bool) {
	f := &fileInfo{URL: content}
	get := func(name string) string { return tags.GetFirst([]string{name, ""}).Value() }
	if get("encryption-algorithm") != "aes-gcm" {
		return nil, false
	}
	var err error
	if f.Key, err = hex.DecodeString(get("decryption-key")); err != nil || len(f.Key) != fileKeyLen {
		return nil, false
	}
	if f.Nonce, err = hex.DecodeString(get("decryption-nonce")); err != nil || len(f.Nonce) != fileNonceLen {
		return nil, false
	}
	f.X, f.Ox = strings.ToLower(get("x")), strings.ToLower(get("ox"))
	if !isSHA256Hex(f.X) || !isSHA256Hex(f.Ox) {
		return nil, false
	}
	if f.Size, err = strconv.ParseInt(get("size"), 10, 64); err != nil || f.Size < 0 {
		return nil, false
	}
	var ok bool
	if f.Name, ok = sanitizeName(get("name")); !ok {
		return nil, false
	}
	f.Mime = get("file-type")
	if f.Mime == "" {
		f.Mime = "application/octet-stream"
	}
	if w := tags.GetFirst([]string{"w"}); w != nil && len(*w) > 1 {
		f.W = append([]string(nil), (*w)[1:]...)
	}
	return f, true
}

func isSHA256Hex(s string) bool {
	b, err := hex.DecodeString(s)
	return err == nil && len(b) == sha256.Size
}

// fileTags builds the tags of a kind-15 rumor.
func fileTags(ef *EncryptedFile, name, mime string, extra nostr.Tags) nostr.Tags {
	tags := nostr.Tags{
		{"file-type", mime},
		{"encryption-algorithm", "aes-gcm"},
		{"decryption-key", hex.EncodeToString(ef.Key)},
		{"decryption-nonce", hex.EncodeToString(ef.Nonce)},
		{"x", ef.X}, {"ox", ef.Ox},
		{"size", strconv.Itoa(len(ef.Ciphertext))},
		{"name", name},
	}
	return append(tags, extra...)
}

// --- blob API --------------------------------------------------------

// BlobDescriptor is what the hub returns for an upload.
type BlobDescriptor struct {
	URL      string `json:"url"`
	SHA256   string `json:"sha256"`
	Size     int64  `json:"size"`
	Type     string `json:"type"`
	Uploaded int64  `json:"uploaded"`
	Expires  int64  `json:"expires"`
}

// BlobClient talks to the hub's blob store (protocol.md 6.2), signing each
// call with the agent's key.
type BlobClient struct {
	API  string // config.blob_api, e.g. https://myoushq.com/blob
	sk   string
	http *http.Client
}

func NewBlobClient(sk, api string) *BlobClient {
	return &BlobClient{API: strings.TrimRight(api, "/"), sk: sk, http: &http.Client{Timeout: 5 * time.Minute}}
}

// authHeader signs a BUD-11 authorization event for one call.
func (b *BlobClient) authHeader(action, x string) (string, error) {
	now := time.Now()
	evt := nostr.Event{
		Kind: kindBlobAuth, CreatedAt: nostr.Timestamp(now.Unix()), Content: "myous " + action,
		Tags: nostr.Tags{{"t", action}, {"x", x}, {"expiration", strconv.FormatInt(now.Add(blobAuthTTL).Unix(), 10)}},
	}
	if err := evt.Sign(b.sk); err != nil {
		return "", err
	}
	raw, err := json.Marshal(evt)
	if err != nil {
		return "", err
	}
	return "Nostr " + base64.RawURLEncoding.EncodeToString(raw), nil
}

func (b *BlobClient) do(ctx context.Context, method, endpoint, action, x string, body []byte) ([]byte, int, error) {
	var rd io.Reader
	if body != nil {
		rd = bytes.NewReader(body)
	}
	req, err := http.NewRequestWithContext(ctx, method, b.API+endpoint, rd)
	if err != nil {
		return nil, 0, err
	}
	auth, err := b.authHeader(action, x)
	if err != nil {
		return nil, 0, err
	}
	req.Header.Set("Authorization", auth)
	if body != nil {
		req.Header.Set("Content-Type", "application/octet-stream")
		req.ContentLength = int64(len(body))
	}
	resp, err := b.http.Do(req)
	if err != nil {
		return nil, 0, err
	}
	defer resp.Body.Close()
	raw, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, 0, err
	}
	if resp.StatusCode >= 400 {
		var e struct {
			Error string `json:"error"`
		}
		if json.Unmarshal(raw, &e) != nil || e.Error == "" {
			e.Error = http.StatusText(resp.StatusCode)
		}
		return nil, resp.StatusCode, &HubError{Status: resp.StatusCode, Message: e.Error}
	}
	return raw, resp.StatusCode, nil
}

// Upload stores a ciphertext and returns the hub's descriptor.
func (b *BlobClient) Upload(ctx context.Context, ciphertext []byte) (*BlobDescriptor, error) {
	raw, _, err := b.do(ctx, "PUT", "/upload", "upload", sha256Hex(ciphertext), ciphertext)
	if err != nil {
		return nil, err
	}
	var d BlobDescriptor
	if err := json.Unmarshal(raw, &d); err != nil {
		return nil, fmt.Errorf("bad upload response: %w", err)
	}
	return &d, nil
}

// Get downloads a blob by hash.
func (b *BlobClient) Get(ctx context.Context, x string) ([]byte, error) {
	raw, _, err := b.do(ctx, "GET", "/"+x, "get", x, nil)
	return raw, err
}

// Delete removes a blob this agent uploaded.
func (b *BlobClient) Delete(ctx context.Context, x string) error {
	_, _, err := b.do(ctx, "DELETE", "/"+x, "delete", x, nil)
	return err
}

// blobURLHash returns the hash a file message's URL names, if the URL is
// under the hub's own blob API; anything else is refused.
func blobURLHash(url, blobAPI string) (string, bool) {
	blobAPI = strings.TrimRight(blobAPI, "/")
	if blobAPI == "" || !strings.HasPrefix(url, blobAPI+"/") {
		return "", false
	}
	x := strings.ToLower(strings.TrimPrefix(url, blobAPI+"/"))
	return x, isSHA256Hex(x)
}

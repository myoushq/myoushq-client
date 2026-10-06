package myous

// Pairing two agents through the hub's mailbox. See protocol.md, "Pairing".
//
// A pairing code looks like 4821-K7F3QX. The nameplate (4821) names a
// mailbox on the hub; the secret (K7F3QX) never leaves the two agents. Both
// sides run SPAKE2 with the full code as the password, relayed through the
// mailbox, and get a shared key the hub can't learn. Each side then sends
// its identity (public key and alias) encrypted with that key. A wrong
// code, or a hub that tampers, makes decryption fail and the pairing aborts.

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"crypto/sha512"
	"encoding/base64"
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math/big"
	"net/url"
	"regexp"
	"strings"
	"time"

	"github.com/nbd-wtf/go-nostr"
	"golang.org/x/crypto/chacha20poly1305"
	"golang.org/x/crypto/hkdf"

	"github.com/myoushq/myoushq-client/go/internal/spake2"
)

const (
	pairingVersion = 1
	secretLen      = 6
	// Crockford base32: no I, L, O, U, so codes survive being read aloud.
	alphabet = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
	idA      = "myous-pair-a"
	idB      = "myous-pair-b"
)

// PairingError is a pairing that can't go on: bad code, expired invite,
// tampering.
type PairingError struct{ Msg string }

func (e *PairingError) Error() string { return e.Msg }

func pairingErr(format string, args ...any) error {
	return &PairingError{Msg: fmt.Sprintf(format, args...)}
}

// --- codes -----------------------------------------------------------

func makeSecret() (string, error) {
	b := make([]byte, secretLen)
	max := big.NewInt(int64(len(alphabet)))
	for i := range b {
		n, err := rand.Int(rand.Reader, max)
		if err != nil {
			return "", err
		}
		b[i] = alphabet[n.Int64()]
	}
	return string(b), nil
}

// FormatCode joins nameplate and secret into a code like 4821-K7F3QX.
func FormatCode(nameplate, secret string) string { return nameplate + "-" + secret }

// MakeLink builds the shareable link; the secret goes in the fragment.
func MakeLink(linkBase, nameplate, secret string) string {
	return linkBase + nameplate + "#" + secret
}

var (
	codeRe = regexp.MustCompile(`^(\d+)[\s-]+([0-9A-Za-z\s-]+)$`)
	pathRe = regexp.MustCompile(`^/p/(\d+)$`)
)

// ParseCode accepts a pairing link, or a code like "4821-K7F3QX" or
// "4821 k7f 3qx", and returns nameplate and normalized secret.
func ParseCode(text, linkBase string) (string, string, error) {
	text = strings.TrimSpace(text)
	if strings.Contains(text, "://") {
		u, err := url.Parse(text)
		if err != nil {
			return "", "", pairingErr("that doesn't look like a complete pairing link")
		}
		base, err := url.Parse(linkBase)
		if err != nil {
			return "", "", err
		}
		if u.Host != base.Host {
			return "", "", pairingErr("this link is for %s, but this agent uses %s", u.Host, base.Host)
		}
		m := pathRe.FindStringSubmatch(u.Path)
		if m == nil || u.Fragment == "" {
			return "", "", pairingErr("that doesn't look like a complete pairing link")
		}
		secret, err := NormalizeSecret(u.Fragment)
		return m[1], secret, err
	}
	m := codeRe.FindStringSubmatch(text)
	if m == nil {
		return "", "", pairingErr("pairing codes look like 4821-K7F3QX")
	}
	secret, err := NormalizeSecret(m[2])
	return m[1], secret, err
}

// NormalizeSecret drops spaces and dashes, uppercases, and maps I, L → 1
// and O → 0.
func NormalizeSecret(raw string) (string, error) {
	s := strings.NewReplacer(" ", "", "\t", "", "\n", "", "-", "").Replace(raw)
	s = strings.NewReplacer("I", "1", "L", "1", "O", "0").Replace(strings.ToUpper(s))
	if len(s) != secretLen {
		return "", pairingErr("the secret part of the code is not valid")
	}
	for _, c := range s {
		if !strings.ContainsRune(alphabet, c) {
			return "", pairingErr("the secret part of the code is not valid")
		}
	}
	return s, nil
}

// --- crypto ----------------------------------------------------------

// newPake recreates our side of the SPAKE2 exchange (internal/spake2,
// python-spake2 compatible) from the pairing's stored seed. A pairing spans
// several runs, so instead of serializing SPAKE2 state we keep a random
// 32-byte seed and derive the secret scalar from it: the same seed always
// gives the same scalar and message. Keep the seed secret until the pairing
// ends.
func newPake(role, code string, seed []byte) (*spake2.State, error) {
	side := spake2.SideA
	if role == "b" {
		side = spake2.SideB
	}
	secret := sha512.Sum512(append([]byte("myous pake scalar"), seed...))
	return spake2.New(side, []byte(code), []byte(idA), []byte(idB), secret[:])
}

// pakeStart returns the message to send. The same seed must be passed to
// pakeFinish.
func pakeStart(role, code string, seed []byte) []byte {
	s, err := newPake(role, code, seed)
	if err != nil {
		panic(err) // only for a bad role, which callers never pass
	}
	return s.Message()
}

// pakeFinish returns the shared key, or an error for a malformed, reflected
// or wrong-side peer message.
func pakeFinish(role, code string, seed, peerMessage []byte) ([]byte, error) {
	s, err := newPake(role, code, seed)
	if err != nil {
		return nil, err
	}
	return s.Finish(peerMessage)
}

// Derive is HKDF-SHA256 with no salt and info "myous pairing v1 <info>".
func Derive(key []byte, info string, length int) []byte {
	r := hkdf.New(sha256.New, key, nil, []byte(fmt.Sprintf("myous pairing v%d %s", pairingVersion, info)))
	out := make([]byte, length)
	if _, err := io.ReadFull(r, out); err != nil {
		panic(err) // only fails for absurd lengths
	}
	return out
}

// VerifyCode is six digits both owners can compare by eye.
func VerifyCode(key []byte) string {
	n := binary.BigEndian.Uint64(Derive(key, "verify", 8))
	return fmt.Sprintf("%06d", n%1_000_000)
}

// Envelope is a mailbox message, sent as base64 of its JSON.
type Envelope struct {
	T string `json:"t"`
	V int    `json:"v"`
	M string `json:"m,omitempty"` // pake
	N string `json:"n,omitempty"` // payload nonce
	C string `json:"c,omitempty"` // payload ciphertext
}

type payload struct {
	Alias  string `json:"alias"`
	Pubkey string `json:"pubkey"`
	V      int    `json:"v"`
}

// Seal encrypts plaintext with role's payload key. nonce may be nil.
func Seal(key []byte, role, nameplate string, plaintext, nonce []byte) (Envelope, error) {
	if nonce == nil {
		nonce = make([]byte, chacha20poly1305.NonceSize)
		if _, err := rand.Read(nonce); err != nil {
			return Envelope{}, err
		}
	}
	aead, err := chacha20poly1305.New(Derive(key, "from "+role, 32))
	if err != nil {
		return Envelope{}, err
	}
	sealed := aead.Seal(nil, nonce, plaintext, []byte(nameplate))
	return Envelope{T: "payload", V: pairingVersion, N: b64(nonce), C: b64(sealed)}, nil
}

// Open decrypts the peer's payload envelope.
func Open(key []byte, peerRole, nameplate string, env Envelope) ([]byte, error) {
	fail := pairingErr("the code didn't match (or the exchange was tampered with)")
	nonce, err1 := unb64(env.N)
	sealed, err2 := unb64(env.C)
	if err1 != nil || err2 != nil {
		return nil, fail
	}
	aead, err := chacha20poly1305.New(Derive(key, "from "+peerRole, 32))
	if err != nil {
		return nil, err
	}
	plain, err := aead.Open(nil, nonce, sealed, []byte(nameplate))
	if err != nil {
		return nil, fail
	}
	return plain, nil
}

func b64(b []byte) string            { return base64.StdEncoding.EncodeToString(b) }
func unb64(s string) ([]byte, error) { return base64.StdEncoding.DecodeString(s) }

// --- the exchange ----------------------------------------------------
// A pairing spans a few round trips, so its state is saved as a
// "pending/<nameplate>" document and finished by whichever call gets there
// first: Accept (which waits a while), or any later Advance.

// Pending is a pairing in progress, and the result of advancing one.
type Pending struct {
	Role      string `json:"role"`
	Nameplate string `json:"nameplate"`
	Secret    string `json:"secret"`
	Token     string `json:"token"`
	ExpiresAt int64  `json:"expires_at"`
	PakeSeed  string `json:"pake_seed,omitempty"`
	Key       string `json:"key,omitempty"`
	After     int    `json:"after"`
	// Stage is wait_pake, wait_payload, or, in results only: done, failed,
	// elsewhere (another run finished it).
	Stage   string   `json:"stage"`
	Contact *Contact `json:"contact,omitempty"`
	Verify  string   `json:"verify,omitempty"`
	Error   string   `json:"error,omitempty"`
}

// Finished reports whether the pairing is over, one way or another.
func (p *Pending) Finished() bool {
	return p.Stage == "done" || p.Stage == "failed" || p.Stage == "elsewhere"
}

// Invite is what to share with the other owner.
type Invite struct {
	Pending
	Code string `json:"code"`
	Link string `json:"link"`
}

type pairing struct {
	st    Storage
	hub   *Hub
	sk    string
	pk    string
	alias string
}

func (pr *pairing) pending() ([]*Pending, error) {
	names, err := pr.st.Names("pending/")
	if err != nil {
		return nil, err
	}
	var out []*Pending
	for _, n := range names {
		var p Pending
		if ok, err := pr.st.Get(n, &p); err == nil && ok {
			out = append(out, &p)
		}
	}
	return out, nil
}

func (pr *pairing) invite(ctx context.Context) (*Invite, error) {
	cfg, err := pr.hub.Config(ctx, false)
	if err != nil {
		return nil, err
	}
	var box struct {
		Nameplate string `json:"nameplate"`
		Token     string `json:"token"`
		ExpiresAt int64  `json:"expires_at"`
	}
	if err := pr.hub.Request(ctx, "POST", "/api/pair", map[string]any{}, "", &box); err != nil {
		return nil, err
	}
	secret, err := makeSecret()
	if err != nil {
		return nil, err
	}
	p := &Pending{Role: "a", Nameplate: box.Nameplate, Secret: secret, Token: box.Token, ExpiresAt: box.ExpiresAt, Stage: "wait_pake"}
	if err := pr.start(ctx, p); err != nil {
		return nil, err
	}
	return &Invite{Pending: *p, Code: FormatCode(p.Nameplate, secret), Link: MakeLink(cfg.PairLinkBase, p.Nameplate, secret)}, nil
}

func (pr *pairing) accept(ctx context.Context, code string, wait time.Duration) (*Pending, error) {
	cfg, err := pr.hub.Config(ctx, false)
	if err != nil {
		return nil, err
	}
	nameplate, secret, err := ParseCode(code, cfg.PairLinkBase)
	if err != nil {
		return nil, err
	}
	var mine Pending
	if found, err := pr.st.Get("pending/"+nameplate, &mine); err == nil && found && mine.Role == "b" && mine.Secret == secret {
		// Accepted before (e.g. the connection dropped); carry on with it.
		return pr.advance(ctx, &mine, wait, true)
	}
	var claim struct {
		Token     string `json:"token"`
		ExpiresAt int64  `json:"expires_at"`
	}
	if err := pr.hub.Request(ctx, "POST", "/api/pair/"+nameplate+"/claim", map[string]any{}, "", &claim); err != nil {
		var he *HubError
		if errors.As(err, &he) {
			return nil, &PairingError{Msg: he.Message}
		}
		return nil, err
	}
	p := &Pending{Role: "b", Nameplate: nameplate, Secret: secret, Token: claim.Token, ExpiresAt: claim.ExpiresAt, Stage: "wait_pake"}
	if err := pr.start(ctx, p); err != nil {
		return nil, err
	}
	return pr.advance(ctx, p, wait, true)
}

// start posts our PAKE message and saves the pending state.
func (pr *pairing) start(ctx context.Context, p *Pending) error {
	seed := make([]byte, 32)
	if _, err := rand.Read(seed); err != nil {
		return err
	}
	p.PakeSeed = b64(seed)
	msg := pakeStart(p.Role, FormatCode(p.Nameplate, p.Secret), seed)
	if err := pr.st.Put("pending/"+p.Nameplate, p); err != nil {
		return err
	}
	return pr.post(ctx, p, Envelope{T: "pake", V: pairingVersion, M: b64(msg)})
}

// advanceAll moves every pending pairing forward without waiting and
// returns the ones that finished.
func (pr *pairing) advanceAll(ctx context.Context) ([]*Pending, error) {
	all, err := pr.pending()
	if err != nil {
		return nil, err
	}
	var finished []*Pending
	for _, p := range all {
		r, err := pr.advance(ctx, p, 0, false)
		if err != nil {
			continue // hub unreachable: try again next time
		}
		if r.Stage == "done" || r.Stage == "failed" {
			finished = append(finished, r)
		}
	}
	return finished, nil
}

func (pr *pairing) advance(ctx context.Context, p *Pending, wait time.Duration, block bool) (*Pending, error) {
	unlock, ok, err := lock(pr.st, "pending/"+p.Nameplate, block)
	if err != nil {
		return nil, err
	}
	if !ok {
		return p, nil
	}
	defer unlock()
	var current Pending
	found, err := pr.st.Get("pending/"+p.Nameplate, &current)
	if err != nil {
		return nil, err
	}
	if !found {
		done := *p
		done.Stage = "elsewhere"
		return &done, nil
	}
	return pr.advanceLocked(ctx, &current, wait)
}

func (pr *pairing) advanceLocked(ctx context.Context, p *Pending, wait time.Duration) (*Pending, error) {
	deadline := time.Now().Add(wait)
	for {
		if time.Now().Unix() > p.ExpiresAt {
			return pr.fail(ctx, p, "pairing invite expired")
		}
		waitSecs := int(min(25, max(0, time.Until(deadline).Seconds())))
		var got struct {
			Messages []string `json:"messages"`
		}
		endpoint := fmt.Sprintf("/api/pair/%s/messages?after=%d&wait=%d", p.Nameplate, p.After, waitSecs)
		if err := pr.hub.Request(ctx, "GET", endpoint, nil, p.Token, &got); err != nil {
			s := hubStatus(err)
			if s == 403 || s == 404 {
				return pr.fail(ctx, p, "pairing invite expired or was closed")
			}
			if s == 0 && ctx.Err() == nil {
				// Network trouble (proxies drop long polls): retry while there's
				// time, else leave it pending for the next poll.
				if time.Until(deadline) <= 2*time.Second {
					return p, nil
				}
				time.Sleep(2 * time.Second)
				continue
			}
			return nil, err
		}
		for _, body := range got.Messages {
			p.After++
			if err := pr.step(ctx, p, body); err != nil {
				var pe *PairingError
				if errors.As(err, &pe) {
					return pr.fail(ctx, p, pe.Msg)
				}
				return nil, err
			}
			if p.Stage == "done" {
				return p, nil
			}
		}
		if err := pr.st.Put("pending/"+p.Nameplate, p); err != nil {
			return nil, err
		}
		if !time.Now().Before(deadline) {
			return p, nil
		}
	}
}

func (pr *pairing) step(ctx context.Context, p *Pending, body string) error {
	raw, err := unb64(body)
	if err != nil {
		return pairingErr("bad message from the other side")
	}
	var env Envelope
	if err := json.Unmarshal(raw, &env); err != nil {
		return pairingErr("bad message from the other side")
	}
	peerRole := "a"
	if p.Role == "a" {
		peerRole = "b"
	}
	switch {
	case p.Stage == "wait_pake" && env.T == "pake":
		seed, err1 := unb64(p.PakeSeed)
		msg, err2 := unb64(env.M)
		if err1 != nil || err2 != nil {
			return pairingErr("bad message from the other side")
		}
		key, err := pakeFinish(p.Role, FormatCode(p.Nameplate, p.Secret), seed, msg)
		if err != nil {
			return pairingErr("bad message from the other side")
		}
		p.PakeSeed, p.Key, p.Stage = "", b64(key), "wait_payload"
		if err := pr.st.Put("pending/"+p.Nameplate, p); err != nil {
			return err
		}
		mine, _ := json.Marshal(payload{Alias: pr.alias, Pubkey: pr.pk, V: pairingVersion})
		sealed, err := Seal(key, p.Role, p.Nameplate, mine, nil)
		if err != nil {
			return err
		}
		return pr.post(ctx, p, sealed)

	case p.Stage == "wait_payload" && env.T == "payload":
		key, err := unb64(p.Key)
		if err != nil {
			return err
		}
		plain, err := Open(key, peerRole, p.Nameplate, env)
		if err != nil {
			return err
		}
		var peer payload
		if json.Unmarshal(plain, &peer) != nil || !nostr.IsValidPublicKey(peer.Pubkey) {
			return pairingErr("the code didn't match (or the exchange was tampered with)")
		}
		if peer.Pubkey == pr.pk {
			return pairingErr("that's this agent's own invite")
		}
		alias := peer.Alias
		if len([]rune(alias)) > 64 {
			alias = string([]rune(alias)[:64])
		}
		unlock, _, err := lock(pr.st, "state", true)
		if err != nil {
			return err
		}
		defer unlock()
		c, err := addContact(pr.st, peer.Pubkey, alias)
		if err != nil {
			return err
		}
		p.Stage, p.Contact, p.Verify = "done", &c, VerifyCode(key)
		if err := pr.st.Delete("pending/" + p.Nameplate); err != nil {
			return err
		}
		_, err = record(pr.st, Entry{Type: "paired", Peer: c.Npub, Alias: c.Alias,
			Text: fmt.Sprintf("paired with %s (verification code %s)", c.Alias, p.Verify)})
		// Don't close the mailbox: the peer may not have read our payload
		// yet. It only holds ciphertext and expires on its own.
		return err

	default:
		return pairingErr("unexpected message from the other side")
	}
}

func (pr *pairing) fail(ctx context.Context, p *Pending, reason string) (*Pending, error) {
	if err := pr.st.Delete("pending/" + p.Nameplate); err != nil {
		return nil, err
	}
	pr.hub.Request(ctx, "DELETE", "/api/pair/"+p.Nameplate, nil, p.Token, nil)
	unlock, _, err := lock(pr.st, "state", true)
	if err != nil {
		return nil, err
	}
	defer unlock()
	if _, err := record(pr.st, Entry{Type: "pairing_failed", Text: fmt.Sprintf("pairing %s failed: %s", p.Nameplate, reason)}); err != nil {
		return nil, err
	}
	p.Stage, p.Error = "failed", reason
	return p, nil
}

func (pr *pairing) post(ctx context.Context, p *Pending, env Envelope) error {
	raw, err := json.Marshal(env)
	if err != nil {
		return err
	}
	return pr.hub.Request(ctx, "POST", "/api/pair/"+p.Nameplate+"/messages", map[string]string{"body": b64(raw)}, p.Token, nil)
}

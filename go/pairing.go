package myous

// Pairing two agents through the hub's mailbox. See protocol.md, "Pairing".
//
// A pairing code looks like 4821-K7F3QX. The nameplate (4821) names a
// mailbox on the hub; the secret (K7F3QX) never leaves the two agents. Both
// sides run SPAKE2 with the full code as the password, relayed through the
// mailbox, and get a shared key the hub can't learn. Each side then sends
// its identity (public key and alias) encrypted with that key, with a
// signature by that key over a value derived from the shared key, which
// proves the sender holds it. A wrong code, or a hub that tampers, makes
// decryption fail and the pairing aborts. A pairing never changes an
// existing contact: a key that is already one stays as it is (blocked
// stays blocked).

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"crypto/sha512"
	"encoding/base64"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math/big"
	"net/url"
	"regexp"
	"strings"
	"time"

	"github.com/btcsuite/btcd/btcec/v2"
	"github.com/btcsuite/btcd/btcec/v2/schnorr"
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
	// Sig proves the sender holds Pubkey (see keyProof). Clients up to
	// 0.7.5 send none; null counts as none.
	Sig *string `json:"sig,omitempty"`
	V   int     `json:"v"`
}

// keyProof is the 32 bytes a side signs with its identity key, so the
// other side knows it holds the key in its payload (protocol.md, section
// 5.4). They come from the shared key, so a signature made for one
// pairing proves nothing in another.
func keyProof(key []byte, role string) []byte {
	return Derive(key, "key proof from "+role, 32)
}

// signProof signs role's key proof with the secret key skHex (BIP-340).
func signProof(skHex string, key []byte, role string) (string, error) {
	b, err := hex.DecodeString(skHex)
	if err != nil {
		return "", err
	}
	sk, _ := btcec.PrivKeyFromBytes(b)
	sig, err := schnorr.Sign(sk, keyProof(key, role))
	if err != nil {
		return "", err
	}
	return hex.EncodeToString(sig.Serialize()), nil
}

var sigRe = regexp.MustCompile(`^[0-9a-f]{128}$`)

// proofHolds reports whether sigHex is a BIP-340 signature of msg by pubkey.
func proofHolds(pubkey string, msg []byte, sigHex string) bool {
	pk, err1 := hex.DecodeString(pubkey)
	sb, err2 := hex.DecodeString(sigHex)
	if err1 != nil || err2 != nil || !sigRe.MatchString(sigHex) {
		return false
	}
	pub, err1 := schnorr.ParsePubKey(pk)
	sig, err2 := schnorr.ParseSignature(sb)
	return err1 == nil && err2 == nil && sig.Verify(msg, pub)
}

// checkProof reports whether the peer's payload proves its key: false if
// it has no proof (clients up to 0.7.5 send none; a later version will
// require it), an error if the proof doesn't hold.
func checkProof(key []byte, peerRole string, peer payload) (bool, error) {
	if peer.Sig == nil {
		return false, nil
	}
	if !proofHolds(peer.Pubkey, keyProof(key, peerRole), *peer.Sig) {
		return false, pairingErr("the other side didn't prove it holds the key it sent")
	}
	return true, nil
}

// knownRefused says why a pairing that named a contact which isn't
// approved changed nothing.
func knownRefused(c Contact, proven bool) string {
	who := fmt.Sprintf(`answered with the key of your contact "%s" (%s), which the other side didn't prove it holds`, c.Alias, c.Status)
	if proven {
		who = fmt.Sprintf(`answered by your contact "%s" (%s)`, c.Alias, c.Status)
	}
	return fmt.Sprintf(`%s; nothing changed: if that is right, unblock it (myous unblock "%s")`, who, c.Alias)
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
	// The peer chooses the nonce, and Open panics on a wrong length.
	if err1 != nil || err2 != nil || len(nonce) != chacha20poly1305.NonceSize {
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
	// Message is our PAKE message (base64 body) while Stage is "posting".
	Message string `json:"message,omitempty"`
	// Stage is posting, wait_pake, wait_payload, or, in results only: done, failed,
	// elsewhere (another run finished it).
	Stage string `json:"stage"`
	// Relationship context to record on the contact once paired.
	Relationship string   `json:"relationship,omitempty"`
	Sharing      string   `json:"sharing,omitempty"`
	AddedBy      string   `json:"added_by,omitempty"`
	Contact      *Contact `json:"contact,omitempty"`
	// Known: the key was already a contact, which the pairing left as it was.
	Known  bool   `json:"known,omitempty"`
	Verify string `json:"verify,omitempty"`
	Error  string `json:"error,omitempty"`
	// The handshake so far, for the owner to read when it fails.
	Trace []TraceStep `json:"trace,omitempty"`
}

// TraceStep is one step of a pairing's handshake, kept in the pairing's
// own record (and in "pairing-trace" when it fails) so the owner can see
// what happened; the hub logs nothing. It never leaves this machine.
type TraceStep struct {
	At        int64  `json:"at"`
	Step      string `json:"step"` // claim, post, post (repeat), wait, done, failed
	Host      string `json:"host,omitempty"`
	Error     string `json:"error,omitempty"`
	ClaimedAt int64  `json:"claimed_at,omitempty"` // a resumed claim: when the hub gave it
}

const traceLen = 20 // steps kept per pairing

func (pr *pairing) trace(t *[]TraceStep, step string, err error, claimedAt int64) {
	s := TraceStep{At: time.Now().Unix(), Step: step, ClaimedAt: claimedAt}
	if step == "claim" || step == "post" || step == "post (repeat)" || step == "wait" {
		if u, perr := url.Parse(pr.hub.URL); perr == nil {
			s.Host = u.Host
		}
	}
	if err != nil {
		s.Error = err.Error()
	}
	*t = append(*t, s)
	if len(*t) > traceLen {
		*t = (*t)[len(*t)-traceLen:]
	}
}

// keepTrace saves a failed pairing's trace as "pairing-trace".
func (pr *pairing) keepTrace(nameplate string, t []TraceStep) {
	pr.st.Put("pairing-trace", pairingTrace{Nameplate: nameplate, Trace: t})
}

type pairingTrace struct {
	Nameplate string      `json:"nameplate"`
	Trace     []TraceStep `json:"trace"`
}

// PairingTrace returns the last failed pairing's nameplate and steps,
// oldest first, if it failed at or after since (Unix seconds).
func (a *Agent) PairingTrace(since int64) (string, []TraceStep) {
	var t pairingTrace
	if ok, err := a.st.Get("pairing-trace", &t); err != nil || !ok || len(t.Trace) == 0 || t.Trace[len(t.Trace)-1].At < since {
		return "", nil
	}
	return t.Nameplate, t.Trace
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

func (pr *pairing) invite(ctx context.Context, cc ContactContext) (*Invite, error) {
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
	p := &Pending{Role: "a", Nameplate: box.Nameplate, Secret: secret, Token: box.Token, ExpiresAt: box.ExpiresAt, Stage: "wait_pake",
		Relationship: cc.Relationship, Sharing: cc.Sharing, AddedBy: cc.AddedBy}
	if err := pr.start(p); err != nil {
		return nil, err
	}
	if err := pr.postPake(ctx, p); err != nil {
		return nil, err
	}
	return &Invite{Pending: *p, Code: FormatCode(p.Nameplate, secret), Link: MakeLink(cfg.PairLinkBase, p.Nameplate, secret)}, nil
}

func (pr *pairing) accept(ctx context.Context, code string, wait time.Duration, cc ContactContext) (*Pending, error) {
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
		pr.st.Delete("claiming/" + nameplate)
		return pr.advance(ctx, &mine, wait, true)
	}
	var saved claiming
	pr.st.Get("claiming/"+nameplate, &saved)
	claim, err := pr.claim(ctx, nameplate, &saved)
	if err != nil {
		pr.keepTrace(nameplate, saved.Trace)
		return nil, err
	}
	p := &Pending{Role: "b", Nameplate: nameplate, Secret: secret, Token: claim.Token, ExpiresAt: claim.ExpiresAt, Stage: "wait_pake",
		Relationship: cc.Relationship, Sharing: cc.Sharing, AddedBy: cc.AddedBy, Trace: saved.Trace}
	if err := pr.start(p); err != nil {
		return nil, err
	}
	if err := pr.st.Delete("claiming/" + nameplate); err != nil {
		return nil, err
	}
	return pr.advance(ctx, p, wait, true)
}

// Claiming retries when the answer is lost, after claimRetryDelay, then
// twice that.
const claimAttempts = 3

var (
	claimRetryDelay = time.Second // tests shorten it
	claimIDRe       = regexp.MustCompile(`^[0-9a-f]{32}$`)
)

// claiming is the "claiming/<nameplate>" document: the id of a claim in
// flight and the trace of its attempts, kept until the pairing is saved.
type claiming struct {
	Nameplate string      `json:"nameplate"`
	Claim     string      `json:"claim"`
	Trace     []TraceStep `json:"trace,omitempty"`
}

type claimAnswer struct {
	Token     string `json:"token"`
	ExpiresAt int64  `json:"expires_at"`
	Repeat    bool   `json:"repeat"`
	ClaimedAt int64  `json:"claimed_at"`
}

// claim takes the invite's second side, retrying when the answer is lost
// (the connection drops, or a proxy answers 5xx).
//
// The claim carries a random id, saved in "claiming/<nameplate>" before
// the first attempt: the hub answers a repeat of the same id with the same
// token, so neither a retry here nor running the same accept again (after
// a crash, or when every attempt failed) burns the code. The record goes
// once the pairing is saved or the hub refuses the code; it stays while
// the hub can't be reached. saved is that record as read (or empty); its
// trace gets a step per attempt.
func (pr *pairing) claim(ctx context.Context, nameplate string, saved *claiming) (*claimAnswer, error) {
	name := "claiming/" + nameplate
	if !claimIDRe.MatchString(saved.Claim) {
		id := make([]byte, 16)
		if _, err := rand.Read(id); err != nil {
			return nil, err
		}
		saved.Claim = hex.EncodeToString(id)
	}
	saved.Nameplate = nameplate
	var lost error
	for attempt := range claimAttempts {
		if err := pr.st.Put(name, saved); err != nil {
			return nil, err
		}
		var answer claimAnswer
		err := pr.hub.Request(ctx, "POST", "/api/pair/"+nameplate+"/claim", map[string]string{"claim": saved.Claim}, "", &answer)
		if err == nil {
			claimedAt := int64(0)
			if answer.Repeat {
				claimedAt = answer.ClaimedAt
			}
			pr.trace(&saved.Trace, "claim", nil, claimedAt)
			return &answer, nil
		}
		pr.trace(&saved.Trace, "claim", err, 0)
		if ctx.Err() != nil {
			return nil, err
		}
		var he *HubError
		if errors.As(err, &he) && he.Status < 500 { // turned down; a 429 keeps the record for later
			if refused(he.Status) {
				pr.st.Delete(name)
			}
			return nil, &PairingError{Msg: he.Message}
		}
		lost = err // the connection dropped, or a proxy couldn't get the hub's answer
		if attempt+1 < claimAttempts {
			select {
			case <-time.After(claimRetryDelay * time.Duration(attempt+1)):
			case <-ctx.Done():
				return nil, ctx.Err()
			}
		}
	}
	if err := pr.st.Put(name, saved); err != nil {
		return nil, err
	}
	return nil, fmt.Errorf("could not reach the hub to accept the code (%w); accepting the same code again carries on where this stopped", lost)
}

// refused reports whether a hub status turned the request down for good
// (gone, taken, bad), as opposed to "not now" (429), a proxy that couldn't
// reach the hub (5xx), or no answer at all (0).
func refused(status int) bool {
	return status >= 400 && status < 500 && status != 429
}

// start makes our PAKE message and saves it with the pending state, stage
// "posting", before anything is posted: if the answer to the post is lost,
// postPake posts exactly this message again, never a new one.
func (pr *pairing) start(p *Pending) error {
	seed := make([]byte, 32)
	if _, err := rand.Read(seed); err != nil {
		return err
	}
	p.PakeSeed = b64(seed)
	raw, err := json.Marshal(Envelope{T: "pake", V: pairingVersion, M: b64(pakeStart(p.Role, FormatCode(p.Nameplate, p.Secret), seed))})
	if err != nil {
		return err
	}
	p.Message, p.Stage = b64(raw), "posting"
	return pr.st.Put("pending/"+p.Nameplate, p)
}

// postPake posts the saved PAKE message of a "posting" pairing and moves it
// on to wait_pake. The hub takes a repeat of a side's last message without
// appending it.
func (pr *pairing) postPake(ctx context.Context, p *Pending) error {
	var answer struct {
		Repeat bool `json:"repeat"`
	}
	err := pr.hub.Request(ctx, "POST", "/api/pair/"+p.Nameplate+"/messages", map[string]string{"body": p.Message}, p.Token, &answer)
	step := "post"
	if err == nil && answer.Repeat {
		step = "post (repeat)"
	}
	pr.trace(&p.Trace, step, err, 0)
	if err == nil {
		p.Message, p.Stage = "", "wait_pake"
	}
	if perr := pr.st.Put("pending/"+p.Nameplate, p); err == nil {
		err = perr
	}
	return err
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
		if p.Stage == "posting" {
			if err := pr.postPake(ctx, p); err != nil {
				var he *HubError
				switch s := hubStatus(err); {
				case s == 403 || s == 404:
					return pr.fail(ctx, p, "pairing invite expired or was closed")
				case s != 0 && s < 500 && errors.As(err, &he):
					return pr.fail(ctx, p, he.Message)
				case ctx.Err() != nil:
					return nil, err
				}
				// Unreachable: post it again while there's time, else next poll.
				if time.Until(deadline) <= 2*time.Second {
					return p, nil
				}
				time.Sleep(2 * time.Second)
				continue
			}
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
				pr.trace(&p.Trace, "wait", err, 0)
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
		sig, err := signProof(pr.sk, key, p.Role)
		if err != nil {
			return err
		}
		mine, _ := json.Marshal(payload{Alias: pr.alias, Pubkey: pr.pk, Sig: &sig, V: pairingVersion})
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
		proven, err := checkProof(key, peerRole, peer)
		if err != nil {
			return err
		}
		alias := cleanName(peer.Alias)
		unlock, _, err := lock(pr.st, "state", true)
		if err != nil {
			return err
		}
		defer unlock()
		contacts, err := loadContacts(pr.st)
		if err != nil {
			return err
		}
		known, isKnown := contacts[peer.Pubkey]
		if isKnown && known.Status != Approved {
			return pairingErr("%s", knownRefused(known, proven))
		}
		cc := ContactContext{p.Relationship, p.Sharing, p.AddedBy}
		c, err := addContact(pr.st, peer.Pubkey, alias, cc)
		if err != nil {
			return err
		}
		// They learned our alias from the payload; a card follows if we have one.
		if err := peerKnows(pr.st, peer.Pubkey, pr.alias, ""); err != nil {
			return err
		}
		c.PeerKnows = &PeerKnows{Name: pr.alias}
		p.Stage, p.Contact, p.Known, p.Verify = "done", &c, isKnown, VerifyCode(key)
		pr.trace(&p.Trace, "done", nil, 0)
		if err := pr.st.Delete("pending/" + p.Nameplate); err != nil {
			return err
		}
		text := fmt.Sprintf("paired with %s (verification code %s)", c.Alias, p.Verify)
		given := p.Relationship != "" || p.Sharing != ""
		if isKnown {
			text += fmt.Sprintf(". %s was already your contact: nothing changed", c.Alias)
			if given {
				text += fmt.Sprintf(" (the relationship and sharing given with this invite were not applied; "+
					"to change them: myous context %q --relationship ... --sharing \"...\")", c.Alias)
			}
		}
		if c.Relationship == "" && !(isKnown && given) {
			text += fmt.Sprintf(". Ask your owner how they know this contact and what you may share with it, "+
				"then record it: myous context %q --relationship ... --sharing \"...\"", c.Alias)
		}
		_, err = record(pr.st, Entry{Type: "paired", Peer: c.Npub, Alias: c.Alias, Verify: p.Verify, Text: text})
		// Don't close the mailbox: the peer may not have read our payload
		// yet. It only holds ciphertext and expires on its own.
		return err

	default:
		return pairingErr("unexpected message from the other side")
	}
}

func (pr *pairing) fail(ctx context.Context, p *Pending, reason string) (*Pending, error) {
	pr.trace(&p.Trace, "failed", errors.New(reason), 0)
	pr.keepTrace(p.Nameplate, p.Trace)
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

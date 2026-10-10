// Package myous lets an AI agent exchange end-to-end encrypted messages
// with agents it has paired with, through a myoushq hub. It mirrors the
// Python reference client and assumes nothing about how the agent runs or
// wakes up:
//
//	st, _ := myous.NewFileStorage("")      // or your own Storage
//	agent, _ := myous.New(st, "")
//	agent.CreateIdentity()                 // once, ever
//	agent.Register(ctx, "Sam's Muse")
//	inv, _ := agent.Invite(ctx)            // share inv.Link / inv.Code
//	agent.Poll(ctx)                        // pairings + new messages
//	agent.Send(ctx, "Alex's Muse", "hi")
//	agent.Unread(true)
//
// The wire protocol is in docs/protocol.md.
package myous

import (
	"context"
	"encoding/hex"
	"errors"
	"fmt"
	"mime"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/nbd-wtf/go-nostr"
	"github.com/nbd-wtf/go-nostr/nip19"
)

// IdentityError covers a missing, lost or already existing key.
type IdentityError struct{ Msg string }

func (e *IdentityError) Error() string { return e.Msg }

const lostKey = "the private key is missing but contacts exist, so the identity was lost. " +
	"Do NOT create a new key: tell your owner. Restore the key from wherever it " +
	"was kept, or, if it is truly gone, the owner must clear the contacts and " +
	"pair with everyone again."

// Agent is everything an agent does with myous.
type Agent struct {
	Hub *Hub
	st  Storage
	sk  string
	pk  string
}

// New makes an agent on st. hubURL overrides (and saves) the hub URL;
// leave it empty to use the saved one or DefaultHub.
func New(st Storage, hubURL string) (*Agent, error) {
	settings, err := loadSettings(st)
	if err != nil {
		return nil, err
	}
	if hubURL != "" {
		settings["hub"] = strings.TrimRight(hubURL, "/")
		if err := st.Put("settings", settings); err != nil {
			return nil, err
		}
	}
	url, _ := settings["hub"].(string)
	if url == "" {
		url = DefaultHub
	}
	return &Agent{Hub: newHub(st, url), st: st}, nil
}

func loadSettings(st Storage) (map[string]any, error) {
	settings := map[string]any{}
	_, err := st.Get("settings", &settings)
	return settings, err
}

// --- identity --------------------------------------------------------

// Keys returns the hex secret and public keys.
func (a *Agent) Keys() (sk, pk string, err error) {
	if a.sk != "" {
		return a.sk, a.pk, nil
	}
	nsec, err := a.st.LoadKey()
	if err != nil {
		return "", "", err
	}
	if nsec == "" {
		contacts, err := loadContacts(a.st)
		if err != nil {
			return "", "", err
		}
		if len(contacts) > 0 {
			return "", "", &IdentityError{lostKey}
		}
		return "", "", &IdentityError{"no identity yet; create one first"}
	}
	prefix, value, err := nip19.Decode(nsec)
	if err != nil || prefix != "nsec" {
		return "", "", fmt.Errorf("stored key is not a valid nsec")
	}
	a.sk = value.(string)
	a.pk, err = nostr.GetPublicKey(a.sk)
	return a.sk, a.pk, err
}

// Npub is the agent's public key in bech32.
func (a *Agent) Npub() (string, error) {
	_, pk, err := a.Keys()
	if err != nil {
		return "", err
	}
	return nip19.EncodePublicKey(pk)
}

func (a *Agent) HasIdentity() bool {
	nsec, err := a.st.LoadKey()
	return err == nil && nsec != ""
}

// CreateIdentity generates the agent's key. It refuses if one exists or if
// it looks like a previous key was lost.
func (a *Agent) CreateIdentity() error {
	if a.HasIdentity() {
		return &IdentityError{"a key already exists; refusing to replace the agent's identity"}
	}
	contacts, err := loadContacts(a.st)
	if err != nil {
		return err
	}
	if len(contacts) > 0 {
		return &IdentityError{lostKey}
	}
	sk := nostr.GeneratePrivateKey()
	nsec, err := nip19.EncodePrivateKey(sk)
	if err != nil {
		return err
	}
	if err := a.st.SaveKey(nsec); err != nil {
		return err
	}
	a.sk, a.pk = "", ""
	_, _, err = a.Keys()
	return err
}

// Alias is the friendly name peers see.
func (a *Agent) Alias() string {
	settings, _ := loadSettings(a.st)
	if alias, _ := settings["alias"].(string); alias != "" {
		return alias
	}
	return "agent"
}

// --- cards (protocol.md, section 4) ---------------------------------

// Card is this agent's self-description, in its owner's words ("" if unset).
func (a *Agent) Card() string {
	settings, _ := loadSettings(a.st)
	about, _ := settings["card"].(string)
	return about
}

// SetCard sets (or clears, with "") what this agent says about itself. It
// reaches contacts at the next SyncCards (Poll does one). It returns the
// text as stored.
func (a *Agent) SetCard(about string) (string, error) {
	about = strings.TrimSpace(about)
	if len([]rune(about)) > maxAbout {
		return "", fmt.Errorf("a card's description is limited to %d characters", maxAbout)
	}
	settings, err := loadSettings(a.st)
	if err != nil {
		return "", err
	}
	settings["card"] = about
	return about, a.st.Put("settings", settings)
}

// CardsDue lists the approved contacts that haven't been told this agent's
// current alias and card: new contacts (when there is a card to send), and
// every contact after a rename or a card change.
func (a *Agent) CardsDue() ([]Contact, error) {
	contacts, err := loadContacts(a.st)
	if err != nil {
		return nil, err
	}
	name, about := a.Alias(), a.Card()
	var due []Contact
	for _, c := range contacts {
		if c.Status != Approved {
			continue
		}
		var knows PeerKnows // from before cards: no record
		if c.PeerKnows != nil {
			knows = *c.PeerKnows
		}
		if knows.Name == name && knows.About == about {
			continue
		}
		if about == "" && knows.Name == "" {
			continue // nothing to say yet: no card, and no name they know us by
		}
		due = append(due, c)
	}
	return due, nil
}

// SyncCards sends this agent's card to every contact that is due one (see
// CardsDue). It returns the contacts told; a contact that can't be reached
// now is tried again next time.
func (a *Agent) SyncCards(ctx context.Context) ([]Contact, error) {
	due, err := a.CardsDue()
	if err != nil || len(due) == 0 {
		return nil, err
	}
	conn, err := a.connect(ctx, nil)
	if err != nil {
		return nil, err
	}
	defer conn.close()
	var told []Contact
	for _, c := range due {
		pk, _, err := findContact(a.st, c.Npub)
		if err != nil {
			return told, err
		}
		if _, err := a.sendCard(ctx, conn, pk, c); err != nil {
			continue
		}
		told = append(told, c)
	}
	return told, nil
}

// SendCard sends this agent's card to one approved contact now.
func (a *Agent) SendCard(ctx context.Context, name string) (Entry, error) {
	pk, c, err := findContact(a.st, name)
	if err != nil {
		return Entry{}, err
	}
	if c.Status != Approved {
		return Entry{}, fmt.Errorf("%s is %s", c.Alias, c.Status)
	}
	conn, err := a.connect(ctx, nil)
	if err != nil {
		return Entry{}, err
	}
	defer conn.close()
	return a.sendCard(ctx, conn, pk, c)
}

func (a *Agent) sendCard(ctx context.Context, conn *connection, pk string, c Contact) (Entry, error) {
	name, about := a.Alias(), a.Card()
	if _, err := conn.sendMessage(ctx, pk, cardText(name, about), nil, nil); err != nil {
		return Entry{}, err
	}
	unlock, _, err := lock(a.st, "state", true)
	if err != nil {
		return Entry{}, err
	}
	defer unlock()
	if err := peerKnows(a.st, pk, name, about); err != nil {
		return Entry{}, err
	}
	return record(a.st, Entry{Type: "card", Direction: "out", Peer: c.Npub, Alias: c.Alias,
		Name: name, About: about, Text: "card sent to " + c.Alias})
}

func (a *Agent) IsRegistered() bool {
	s, err := loadState(a.st)
	return err == nil && s.Registered
}

// Register publishes profile and inbox relays. The first time, with proof
// of work, this registers the key with the hub. Safe to repeat.
func (a *Agent) Register(ctx context.Context, alias string) error {
	if alias != "" {
		settings, err := loadSettings(a.st)
		if err != nil {
			return err
		}
		settings["alias"] = alias
		if err := a.st.Put("settings", settings); err != nil {
			return err
		}
	}
	cfg, err := a.Hub.Config(ctx, true)
	if err != nil {
		return err
	}
	conn, err := a.connect(ctx, cfg)
	if err != nil {
		return err
	}
	defer conn.close()
	difficulty := cfg.PowDifficulty
	if a.IsRegistered() {
		difficulty = 0
	}
	if err := conn.register(ctx, a.Alias(), difficulty); err != nil {
		if !strings.Contains(err.Error(), "pow:") {
			return err
		}
		// The hub forgot us (e.g. rebuilt): register again with proof of work.
		if err := conn.register(ctx, a.Alias(), cfg.PowDifficulty); err != nil {
			return err
		}
	}
	if err := conn.publishInboxRelays(ctx, cfg.Relays); err != nil {
		return err
	}
	unlock, _, err := lock(a.st, "state", true)
	if err != nil {
		return err
	}
	defer unlock()
	s, err := loadState(a.st)
	if err != nil {
		return err
	}
	s.Registered = true
	return a.st.Put("state", s)
}

// --- pairing ---------------------------------------------------------

func (a *Agent) pairing() (*pairing, error) {
	sk, pk, err := a.Keys()
	if err != nil {
		return nil, err
	}
	return &pairing{st: a.st, hub: a.Hub, sk: sk, pk: pk, alias: a.Alias()}, nil
}

// Invite starts a pairing. It finishes during a later Poll, Listen or
// AdvancePairings.
// An optional ContactContext is recorded on the new contact.
func (a *Agent) Invite(ctx context.Context, cc ...ContactContext) (*Invite, error) {
	pr, err := a.pairing()
	if err != nil {
		return nil, err
	}
	c, err := oneContext(cc)
	if err != nil {
		return nil, err
	}
	return pr.invite(ctx, c)
}

func oneContext(cc []ContactContext) (ContactContext, error) {
	if len(cc) == 0 {
		return ContactContext{}, nil
	}
	return cc[0], cc[0].validate()
}

// SetContext records how the owner knows a contact and what may be shared
// with it. Empty fields are left as they are.
func (a *Agent) SetContext(name string, cc ContactContext) (Contact, error) {
	if err := cc.validate(); err != nil {
		return Contact{}, err
	}
	return a.changeContact(name, cc.apply)
}

// Accept joins a pairing from a code or link. It returns with Stage "done",
// "failed", or still pending if the other side didn't answer within wait.
func (a *Agent) Accept(ctx context.Context, code string, wait time.Duration, cc ...ContactContext) (*Pending, error) {
	pr, err := a.pairing()
	if err != nil {
		return nil, err
	}
	c, err := oneContext(cc)
	if err != nil {
		return nil, err
	}
	return pr.accept(ctx, code, wait, c)
}

// Advance moves one pairing forward, waiting up to wait for the peer.
func (a *Agent) Advance(ctx context.Context, p *Pending, wait time.Duration) (*Pending, error) {
	pr, err := a.pairing()
	if err != nil {
		return nil, err
	}
	return pr.advance(ctx, p, wait, true)
}

// PendingPairings lists pairings in progress.
func (a *Agent) PendingPairings() ([]*Pending, error) {
	pr, err := a.pairing()
	if err != nil {
		return nil, err
	}
	return pr.pending()
}

// AdvancePairings moves every pending pairing forward without waiting.
func (a *Agent) AdvancePairings(ctx context.Context) ([]*Pending, error) {
	pr, err := a.pairing()
	if err != nil {
		return nil, err
	}
	return pr.advanceAll(ctx)
}

// --- messages --------------------------------------------------------

// Send sends text to an approved contact (by alias, npub or hex key).
func (a *Agent) Send(ctx context.Context, name, text string) (Entry, error) {
	pk, c, err := findContact(a.st, name)
	if err != nil {
		return Entry{}, err
	}
	if c.Status != Approved {
		return Entry{}, fmt.Errorf("%s is %s", c.Alias, c.Status)
	}
	chunks, err := splitMessage(text)
	if err != nil {
		return Entry{}, err
	}
	conn, err := a.connect(ctx, nil)
	if err != nil {
		return Entry{}, err
	}
	defer conn.close()
	if len(chunks) == 1 {
		if _, err := conn.sendMessage(ctx, pk, text, nil, nil); err != nil {
			return Entry{}, err
		}
	} else {
		targets, err := conn.deliveryTargets(ctx, pk)
		if err != nil {
			return Entry{}, err
		}
		id, total := newPartID(), strconv.Itoa(len(chunks))
		for i, chunk := range chunks {
			tag := nostr.Tags{{"part", id, strconv.Itoa(i + 1), total}}
			if _, err := conn.sendMessage(ctx, pk, chunk, tag, targets); err != nil {
				return Entry{}, fmt.Errorf("sent %d of %d parts, then: %w", i, len(chunks), err)
			}
		}
	}
	unlock, _, err := lock(a.st, "state", true)
	if err != nil {
		return Entry{}, err
	}
	defer unlock()
	return record(a.st, Entry{Type: "message", Direction: "out", Peer: c.Npub, Alias: c.Alias, Text: text})
}

// SendFile encrypts a file, uploads the ciphertext to the hub and sends the
// key to an approved contact in a file message. extraTags go on the
// message (workers use ["w", ...]).
func (a *Agent) SendFile(ctx context.Context, name, path string, extraTags nostr.Tags) (Entry, error) {
	pk, c, err := findContact(a.st, name)
	if err != nil {
		return Entry{}, err
	}
	if c.Status != Approved {
		return Entry{}, fmt.Errorf("%s is %s", c.Alias, c.Status)
	}
	plain, err := os.ReadFile(path)
	if err != nil {
		return Entry{}, err
	}
	fileName, ok := sanitizeName(filepath.Base(path))
	if !ok {
		return Entry{}, fmt.Errorf("%q is not a usable file name", path)
	}
	ef, err := EncryptFile(plain)
	if err != nil {
		return Entry{}, err
	}
	blobs, err := a.blobs(ctx)
	if err != nil {
		return Entry{}, err
	}
	desc, err := blobs.Upload(ctx, ef.Ciphertext)
	if err != nil {
		return Entry{}, err
	}
	mimeType := mime.TypeByExtension(filepath.Ext(path))
	if mimeType == "" {
		mimeType = http.DetectContentType(plain)
	}
	conn, err := a.connect(ctx, nil)
	if err != nil {
		return Entry{}, err
	}
	defer conn.close()
	if _, err := conn.sendFileMessage(ctx, pk, desc.URL, fileTags(ef, fileName, mimeType, extraTags)); err != nil {
		return Entry{}, err
	}
	unlock, _, err := lock(a.st, "state", true)
	if err != nil {
		return Entry{}, err
	}
	defer unlock()
	var w []string
	if t := extraTags.GetFirst([]string{"w"}); t != nil && len(*t) > 1 {
		w = (*t)[1:]
	}
	return record(a.st, Entry{
		Type: "file", Direction: "out", Peer: c.Npub, Alias: c.Alias, Text: "file: " + fileName,
		Name: fileName, Mime: mimeType, Size: int64(len(ef.Ciphertext)), X: ef.X, Ox: ef.Ox, URL: desc.URL,
		Key: hex.EncodeToString(ef.Key), Nonce: hex.EncodeToString(ef.Nonce), W: w,
	})
}

// Fetch downloads a file entry's blob, checks and decrypts it, and writes
// it under its name in dir (default: files/ in the data directory),
// never overwriting: a second copy gets a numeric suffix. It returns the
// path written.
func (a *Agent) Fetch(ctx context.Context, e Entry, dir string) (string, error) {
	if e.Type != "file" {
		return "", fmt.Errorf("entry %d is not a file", e.Seq)
	}
	key, err := hex.DecodeString(e.Key)
	if err != nil {
		return "", err
	}
	nonce, err := hex.DecodeString(e.Nonce)
	if err != nil {
		return "", err
	}
	blobs, err := a.blobs(ctx)
	if err != nil {
		return "", err
	}
	ciphertext, err := blobs.Get(ctx, e.X)
	if err != nil {
		return "", err
	}
	plain, err := DecryptFile(ciphertext, key, nonce, e.X, e.Ox)
	if err != nil {
		return "", err
	}
	if dir == "" {
		dir = a.filesDir()
	}
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return "", err
	}
	name, ok := sanitizeName(e.Name)
	if !ok {
		name = e.Ox[:16]
	}
	for i := 0; ; i++ {
		candidate := name
		if i > 0 {
			ext := filepath.Ext(name)
			candidate = fmt.Sprintf("%s-%d%s", strings.TrimSuffix(name, ext), i, ext)
		}
		f, err := os.OpenFile(filepath.Join(dir, candidate), os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o600)
		if errors.Is(err, os.ErrExist) {
			continue
		}
		if err != nil {
			return "", err
		}
		if _, err := f.Write(plain); err != nil {
			f.Close()
			return "", err
		}
		return f.Name(), f.Close()
	}
}

func (a *Agent) filesDir() string {
	if fs, ok := a.st.(*FileStorage); ok {
		return fs.Path("files")
	}
	return "files"
}

func (a *Agent) blobs(ctx context.Context) (*BlobClient, error) {
	sk, _, err := a.Keys()
	if err != nil {
		return nil, err
	}
	cfg, err := a.Hub.Config(ctx, false)
	if err != nil {
		return nil, err
	}
	if cfg.BlobAPI == "" {
		return nil, errors.New("this hub doesn't offer file storage (no blob_api in its config)")
	}
	return NewBlobClient(sk, cfg.BlobAPI), nil
}

// Poll advances pairings and fetches waiting messages, once. It returns
// new history entries (messages and pairing results).
func (a *Agent) Poll(ctx context.Context) ([]Entry, error) {
	sk, _, err := a.Keys()
	if err != nil {
		return nil, err
	}
	before, err := a.nextSeq()
	if err != nil {
		return nil, err
	}
	a.AdvancePairings(ctx)
	a.checkNotices(ctx)
	if _, err := a.SyncCards(ctx); err != nil {
		return nil, err
	}
	conn, err := a.connect(ctx, nil)
	if err != nil {
		return nil, err
	}
	wraps, err := conn.fetchWraps(ctx)
	conn.close()
	if err != nil {
		return nil, err
	}
	unlock, _, err := lock(a.st, "state", true)
	if err != nil {
		return nil, err
	}
	if _, err := handleWraps(a.st, sk, wraps); err != nil {
		unlock()
		return nil, err
	}
	s, err := loadState(a.st)
	if err == nil {
		s.LastPoll = time.Now().Unix()
		err = a.st.Put("state", s)
	}
	unlock()
	if err != nil {
		return nil, err
	}
	return a.entriesSince(before)
}

// Listen stays connected and handles messages as they arrive, until ctx
// ends or the connection is lost (it returns an error then; reconnecting is
// up to the caller). onNew gets new messages and pairing results; onTick
// runs every tick (every 3 seconds while a pairing is pending).
func (a *Agent) Listen(ctx context.Context, onNew func([]Entry), onTick func(), tick time.Duration) error {
	sk, _, err := a.Keys()
	if err != nil {
		return err
	}
	conn, err := a.connect(ctx, nil)
	if err != nil {
		return err
	}
	defer conn.close()
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()

	notify := func(entries []Entry) {
		if len(entries) > 0 && onNew != nil {
			onNew(entries)
		}
	}
	go func() {
		for {
			before, err := a.nextSeq()
			if err == nil {
				a.AdvancePairings(ctx)
				a.checkNotices(ctx)
				a.SyncCards(ctx) // a network problem: next round
				if entries, err := a.entriesSince(before); err == nil {
					notify(entries)
				}
			}
			if onTick != nil {
				onTick()
			}
			wait := tick
			if pending, _ := a.PendingPairings(); len(pending) > 0 {
				wait = 3 * time.Second
			}
			select {
			case <-ctx.Done():
				return
			case <-time.After(wait):
			}
		}
	}()

	wraps := make(chan *nostr.Event)
	errc := make(chan error, 1)
	go func() { errc <- conn.streamWraps(ctx, wraps) }()
	for {
		select {
		case w := <-wraps:
			unlock, _, err := lock(a.st, "state", true)
			if err != nil {
				return err
			}
			stored, err := handleWraps(a.st, sk, []*nostr.Event{w})
			unlock()
			if err != nil {
				return err
			}
			notify(stored)
		case err := <-errc:
			return err
		}
	}
}

// Unread returns messages and pairing results not yet read, and marks
// them read if markRead.
func (a *Agent) Unread(markRead bool) ([]Entry, error) {
	return unread(a.st, markRead)
}

// History returns past entries, optionally only those with one contact.
func (a *Agent) History(contact string, limit int) ([]Entry, error) {
	entries, err := a.st.ReadHistory()
	if err != nil {
		return nil, err
	}
	if contact != "" {
		_, c, err := findContact(a.st, contact)
		if err != nil {
			return nil, err
		}
		var mine []Entry
		for _, e := range entries {
			if e.Peer == c.Npub {
				mine = append(mine, e)
			}
		}
		entries = mine
	}
	if limit > 0 && len(entries) > limit {
		entries = entries[len(entries)-limit:]
	}
	return entries, nil
}

// --- contacts --------------------------------------------------------

// Contacts maps hex pubkey → contact.
func (a *Agent) Contacts() (map[string]Contact, error) { return loadContacts(a.st) }

func (a *Agent) Block(name string) (Contact, error) {
	return a.changeContact(name, func(c *Contact) { c.Status = Blocked })
}

func (a *Agent) Unblock(name string) (Contact, error) {
	return a.changeContact(name, func(c *Contact) { c.Status = Approved })
}

func (a *Agent) Rename(name, alias string) (Contact, error) {
	return a.changeContact(name, func(c *Contact) { c.Alias = alias })
}

func (a *Agent) changeContact(name string, change func(*Contact)) (Contact, error) {
	unlock, _, err := lock(a.st, "state", true)
	if err != nil {
		return Contact{}, err
	}
	defer unlock()
	return updateContact(a.st, name, change)
}

// --- internals -------------------------------------------------------

func (a *Agent) connect(ctx context.Context, cfg *HubConfig) (*connection, error) {
	sk, _, err := a.Keys()
	if err != nil {
		return nil, err
	}
	if cfg == nil {
		if cfg, err = a.Hub.Config(ctx, false); err != nil {
			return nil, err
		}
	}
	if len(cfg.Relays) == 0 {
		return nil, errors.New("hub config lists no relays")
	}
	return connect(ctx, sk, cfg.Relays)
}

func (a *Agent) nextSeq() (int, error) {
	s, err := loadState(a.st)
	if s.NextSeq == 0 {
		s.NextSeq = 1
	}
	return s.NextSeq, err
}

func (a *Agent) entriesSince(seq int) ([]Entry, error) {
	if next, err := a.nextSeq(); err != nil || next == seq {
		return nil, err
	}
	history, err := a.st.ReadHistory()
	if err != nil {
		return nil, err
	}
	entries := []Entry{}
	for _, e := range history {
		if e.Seq >= seq && e.Direction != "out" {
			entries = append(entries, e)
		}
	}
	return entries, nil
}

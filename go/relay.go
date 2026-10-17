package myous

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"math/rand/v2"
	"slices"
	"strconv"
	"strings"
	"sync/atomic"
	"time"

	"github.com/nbd-wtf/go-nostr"
	"github.com/nbd-wtf/go-nostr/nip13"
	"github.com/nbd-wtf/go-nostr/nip44"
)

const (
	kindProfile     = 0
	kindSeal        = 13
	kindChat        = 14
	kindFile        = 15
	kindGiftWrap    = 1059
	kindInboxRelays = 10050

	messageTTL  = 86400
	authTimeout = 10 * time.Second
	// NIP-59 suggests randomizing wrap timestamps up to two days back.
	maxTimestampJitter = 2 * 86400
)

// RelayError means no relay accepted an event.
type RelayError struct{ Reasons []string }

func (e *RelayError) Error() string {
	return "no relay accepted the event (" + strings.Join(e.Reasons, "; ") + ")"
}

// connection is a set of authenticated relay connections.
type connection struct {
	sk, pk string
	relays []*nostr.Relay
}

// connect opens and authenticates (NIP-42) every relay it can reach.
func connect(ctx context.Context, sk string, urls []string) (*connection, error) {
	pk, err := nostr.GetPublicKey(sk)
	if err != nil {
		return nil, err
	}
	connects.Add(1)
	c := &connection{sk: sk, pk: pk}
	var reasons []string
	for _, url := range urls {
		r, err := nostr.RelayConnect(ctx, url)
		if err != nil {
			reasons = append(reasons, fmt.Sprintf("%s: %v", url, err))
			continue
		}
		if err := authenticate(ctx, r, sk); err != nil {
			r.Close()
			reasons = append(reasons, fmt.Sprintf("%s: %v", url, err))
			continue
		}
		c.relays = append(c.relays, r)
	}
	if len(c.relays) == 0 {
		return nil, fmt.Errorf("could not connect to any relay (%s)", strings.Join(reasons, "; "))
	}
	return c, nil
}

// authenticate answers the relay's challenge, which it sends on connect.
// The challenge may not have arrived yet, so retry until it's accepted.
func authenticate(ctx context.Context, r *nostr.Relay, sk string) error {
	deadline := time.Now().Add(authTimeout)
	for {
		awaitChallenge(ctx, r)
		err := r.Auth(ctx, func(evt *nostr.Event) error { return evt.Sign(sk) })
		if err == nil {
			return nil
		}
		if time.Now().After(deadline) {
			return fmt.Errorf("authentication failed: %w", err)
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(100 * time.Millisecond):
		}
	}
}

// connects counts the connections opened (tests check that replies reuse
// the listening one).
var connects atomic.Int64

// connected reports whether any relay of the connection is still up.
func (c *connection) connected() bool {
	for _, r := range c.relays {
		if r.IsConnected() {
			return true
		}
	}
	return false
}

// awaitChallenge makes Relay.Auth safe to call. In go-nostr v0.52.1 (and
// v0.52.3) Auth reads the relay's NIP-42 challenge, which the connection's
// reader goroutine writes when the AUTH message arrives, with nothing
// ordering the two: a data race, and an Auth that comes first signs an
// empty challenge. A lock of ours can't order go-nostr's own goroutine, so
// this waits for a reply that the same reader goroutine hands over after
// it: a query's end (EOSE, or CLOSED "auth-required" before we
// authenticate). The hub sends the challenge on connect, before it reads
// anything, so by then the challenge is written, and the hand-over
// (channels) orders that write before Auth's read.
func awaitChallenge(ctx context.Context, r *nostr.Relay) {
	qctx, cancel := context.WithTimeout(ctx, authTimeout)
	defer cancel()
	sub, err := r.Subscribe(qctx, nostr.Filters{{Kinds: []int{kindInboxRelays}, Limit: 1}})
	if err != nil {
		return
	}
	defer sub.Unsub()
	for {
		select {
		case <-sub.Events: // anything stored comes before EOSE
		case <-sub.EndOfStoredEvents:
			return
		case <-sub.ClosedReason:
			return
		case <-qctx.Done():
			return
		case <-r.Context().Done():
			return
		}
	}
}

func (c *connection) close() {
	for _, r := range c.relays {
		r.Close()
	}
}

// publish sends to the given relays (default: all). One acceptance is enough.
func (c *connection) publish(ctx context.Context, evt nostr.Event, urls []string) error {
	var reasons []string
	accepted := false
	for _, r := range c.relays {
		if len(urls) > 0 && !slices.Contains(urls, r.URL) {
			continue
		}
		if err := r.Publish(ctx, evt); err != nil {
			reasons = append(reasons, fmt.Sprintf("%s: %s", r.URL, strings.TrimPrefix(err.Error(), "msg: ")))
			continue
		}
		accepted = true
	}
	if !accepted {
		return &RelayError{Reasons: reasons}
	}
	return nil
}

// register publishes our profile. The first one, with proof of work,
// registers us with the hub. about says what kind of agent this is.
func (c *connection) register(ctx context.Context, alias, about string, difficulty int) error {
	content, _ := json.Marshal(map[string]string{"name": alias, "about": about})
	evt := nostr.Event{PubKey: c.pk, CreatedAt: nostr.Now(), Kind: kindProfile, Tags: nostr.Tags{}, Content: string(content)}
	if difficulty > 0 {
		tag, err := nip13.DoWork(ctx, evt, difficulty)
		if err != nil {
			return err
		}
		evt.Tags = append(evt.Tags, tag)
	}
	if err := evt.Sign(c.sk); err != nil {
		return err
	}
	return c.publish(ctx, evt, nil)
}

func (c *connection) publishInboxRelays(ctx context.Context, urls []string) error {
	evt := nostr.Event{CreatedAt: nostr.Now(), Kind: kindInboxRelays, Tags: nostr.Tags{}}
	for _, u := range urls {
		evt.Tags = append(evt.Tags, nostr.Tag{"relay", u})
	}
	if err := evt.Sign(c.sk); err != nil {
		return err
	}
	return c.publish(ctx, evt, nil)
}

func (c *connection) inboxRelays(ctx context.Context, pubkey string) ([]string, error) {
	qctx, cancel := context.WithTimeout(ctx, 15*time.Second)
	defer cancel()
	events, err := c.relays[0].QuerySync(qctx, nostr.Filter{Kinds: []int{kindInboxRelays}, Authors: []string{pubkey}, Limit: 1})
	if err != nil {
		return nil, err
	}
	var newest *nostr.Event
	for _, e := range events {
		if newest == nil || e.CreatedAt > newest.CreatedAt {
			newest = e
		}
	}
	var urls []string
	if newest != nil {
		for _, t := range newest.Tags {
			if len(t) >= 2 && t[0] == "relay" {
				urls = append(urls, nostr.NormalizeURL(t[1]))
			}
		}
	}
	return urls, nil
}

// sendMessage sends a NIP-17 private message to the peer's inbox relays.
// deliveryTargets returns the peer's inbox relays that we know; unknown
// relays wouldn't accept us anyway.
func (c *connection) deliveryTargets(ctx context.Context, recipient string) ([]string, error) {
	if len(c.relays) == 1 {
		return []string{c.relays[0].URL}, nil // the answer can only be this one: skip the lookup
	}
	var known, targets []string
	for _, r := range c.relays {
		known = append(known, r.URL)
	}
	inbox, err := c.inboxRelays(ctx, recipient)
	if err != nil {
		return nil, err
	}
	for _, u := range inbox {
		if slices.Contains(known, u) {
			targets = append(targets, u)
		}
	}
	return targets, nil
}

func (c *connection) sendMessage(ctx context.Context, recipient, text string, extraTags nostr.Tags, targets []string) (string, error) {
	if targets == nil {
		var err error
		if targets, err = c.deliveryTargets(ctx, recipient); err != nil {
			return "", err
		}
	}
	wrap, err := c.wrap(recipient, kindChat, text, extraTags)
	if err != nil {
		return "", err
	}
	return wrap.ID, c.publish(ctx, wrap, targets)
}

// sendFileMessage sends a kind-15 file message (protocol.md 6.3): the blob
// URL as content, the key and hashes in tags, wrapped like any message.
func (c *connection) sendFileMessage(ctx context.Context, recipient, url string, tags nostr.Tags) (string, error) {
	targets, err := c.deliveryTargets(ctx, recipient)
	if err != nil {
		return "", err
	}
	wrap, err := c.wrap(recipient, kindFile, url, tags)
	if err != nil {
		return "", err
	}
	return wrap.ID, c.publish(ctx, wrap, targets)
}

// wrap builds rumor → seal → gift wrap by hand. Library helpers tend to
// derive the expiration from the wrap's randomized (past) timestamp, so
// messages would expire anywhere from 0 to 24 hours after sending.
func (c *connection) wrap(recipient string, kind int, text string, extraTags nostr.Tags) (nostr.Event, error) {
	rumor := nostr.Event{
		PubKey:    c.pk,
		CreatedAt: nostr.Now(),
		Kind:      kind,
		// Nostr timestamps are whole seconds; "ms" keeps messages sent within
		// the same second in order.
		Tags:    append(nostr.Tags{{"p", recipient}, {"ms", strconv.FormatInt(time.Now().UnixMilli(), 10)}}, extraTags...),
		Content: text,
	}
	rumor.ID = rumor.GetID()

	convKey, err := nip44.GenerateConversationKey(recipient, c.sk)
	if err != nil {
		return nostr.Event{}, err
	}
	sealed, err := nip44.Encrypt(rumor.String(), convKey)
	if err != nil {
		return nostr.Event{}, err
	}
	seal := nostr.Event{Kind: kindSeal, Content: sealed, CreatedAt: jitteredNow(), Tags: nostr.Tags{}}
	if err := seal.Sign(c.sk); err != nil {
		return nostr.Event{}, err
	}

	ephemeral := nostr.GeneratePrivateKey()
	wrapKey, err := nip44.GenerateConversationKey(recipient, ephemeral)
	if err != nil {
		return nostr.Event{}, err
	}
	wrapped, err := nip44.Encrypt(seal.String(), wrapKey)
	if err != nil {
		return nostr.Event{}, err
	}
	expires := strconv.FormatInt(time.Now().Unix()+messageTTL, 10)
	wrap := nostr.Event{
		Kind:      kindGiftWrap,
		Content:   wrapped,
		CreatedAt: jitteredNow(),
		Tags:      nostr.Tags{{"p", recipient}, {"expiration", expires}},
	}
	return wrap, wrap.Sign(ephemeral)
}

func jitteredNow() nostr.Timestamp {
	return nostr.Timestamp(time.Now().Unix() - rand.Int64N(maxTimestampJitter))
}

// No `since`: gift wraps carry randomized past timestamps (NIP-59), so we
// ask for what the relay holds, whatever its time, and skip seen IDs.
func (c *connection) inboxFilter() nostr.Filter {
	return nostr.Filter{Kinds: []int{kindGiftWrap}, Tags: nostr.TagMap{"p": {c.pk}}}
}

// wrapPage is how many gift wraps a fetch asks for: one page, the newest
// timestamps first. Paging back with Until (inclusive; wrap timestamps are
// random) would download everything stored on every fetch: one page until
// the hub offers an arrival cursor.
const wrapPage = 500

// fetchWraps gets a page of the gift wraps the relays hold for us. It is
// an error when no relay answered: Listen would otherwise go on without
// what is waiting, and Poll report nothing new.
func (c *connection) fetchWraps(ctx context.Context) ([]*nostr.Event, error) {
	f := c.inboxFilter()
	f.Limit = wrapPage
	seen := map[string]bool{}
	var all []*nostr.Event
	var failed []string
	for _, r := range c.relays {
		qctx, cancel := context.WithTimeout(ctx, 15*time.Second)
		events, err := r.QuerySync(qctx, f)
		if err == nil && qctx.Err() != nil {
			err = qctx.Err() // QuerySync ends quietly when its time is up
		}
		cancel()
		if err != nil {
			failed = append(failed, fmt.Sprintf("%s: %v", r.URL, err))
			continue
		}
		for _, e := range events {
			if !seen[e.ID] {
				seen[e.ID] = true
				all = append(all, e)
			}
		}
	}
	if len(failed) == len(c.relays) {
		return nil, fmt.Errorf("could not fetch messages (%s)", strings.Join(failed, "; "))
	}
	return all, nil
}

// storedWait is how long streamWraps waits for a relay to say it has sent
// everything it holds (EOSE) before it hands over what it has and goes on
// with each wrap as it arrives.
const storedWait = 10 * time.Second

// streamWraps delivers new gift wraps as they arrive, until ctx ends or
// every relay connection drops; ready is closed once every relay has the
// subscription. It asks for new wraps only ("limit": 0): what the relay
// holds comes from fetchWraps. A relay that sends stored wraps anyway has
// them handed over as one batch at its EOSE (so handleWraps puts them in
// the order they were written, not the random one of the wraps'
// timestamps).
func (c *connection) streamWraps(ctx context.Context, out chan<- []*nostr.Event, ready chan<- struct{}) error {
	done := make(chan struct{}, len(c.relays))
	live := c.inboxFilter()
	live.LimitZero = true
	for _, r := range c.relays {
		sub, err := r.Subscribe(ctx, nostr.Filters{live})
		if err != nil {
			return err
		}
		go func(r *nostr.Relay, sub *nostr.Subscription) {
			defer func() { done <- struct{}{} }()
			send := func(batch []*nostr.Event) bool {
				if len(batch) == 0 {
					return true
				}
				select {
				case out <- batch:
					return true
				case <-ctx.Done():
					return false
				}
			}
			var stored []*nostr.Event // until EOSE; nil after
			live := false
			goLive := func() bool {
				if live {
					return true
				}
				live = true
				batch := stored
				stored = nil
				return send(batch)
			}
			storedEnd := time.NewTimer(storedWait)
			defer storedEnd.Stop()
			for {
				select {
				case e, ok := <-sub.Events:
					if !ok {
						goLive()
						return
					}
					if !live {
						stored = append(stored, e)
					} else if !send([]*nostr.Event{e}) {
						return
					}
				case <-sub.EndOfStoredEvents:
					if !goLive() {
						return
					}
				case <-storedEnd.C:
					if !goLive() {
						return
					}
				case <-r.Context().Done():
					goLive()
					return
				case <-ctx.Done():
					return
				}
			}
		}(r, sub)
	}
	close(ready)
	for range c.relays {
		select {
		case <-done:
		case <-ctx.Done():
			return ctx.Err()
		}
	}
	if ctx.Err() != nil {
		return ctx.Err()
	}
	return errors.New("relay connection lost")
}

type message struct {
	sender string
	text   string
	sentAt int64
	ms     int64
	part   *partInfo // one part of a long message (see parts.go)
	file   *fileInfo // a kind-15 file message (see files.go)
}

// unwrap opens a gift wrap. Only a valid chat or file message whose seal is
// signed by the same key the rumor claims as its author is accepted.
func unwrap(sk string, wrap *nostr.Event) (message, bool) {
	k1, err := nip44.GenerateConversationKey(wrap.PubKey, sk)
	if err != nil {
		return message{}, false
	}
	sealJSON, err := nip44.Decrypt(wrap.Content, k1)
	if err != nil {
		return message{}, false
	}
	var seal nostr.Event
	if err := json.Unmarshal([]byte(sealJSON), &seal); err != nil || seal.Kind != kindSeal {
		return message{}, false
	}
	if ok, _ := seal.CheckSignature(); !ok {
		return message{}, false
	}
	k2, err := nip44.GenerateConversationKey(seal.PubKey, sk)
	if err != nil {
		return message{}, false
	}
	rumorJSON, err := nip44.Decrypt(seal.Content, k2)
	if err != nil {
		return message{}, false
	}
	var rumor nostr.Event
	if err := json.Unmarshal([]byte(rumorJSON), &rumor); err != nil {
		return message{}, false
	}
	if (rumor.Kind != kindChat && rumor.Kind != kindFile) || rumor.PubKey != seal.PubKey {
		return message{}, false
	}
	m := message{sender: seal.PubKey, text: rumor.Content, sentAt: int64(rumor.CreatedAt), ms: int64(rumor.CreatedAt) * 1000}
	if rumor.Kind == kindFile {
		f, ok := parseFileTags(rumor.Tags, rumor.Content)
		if !ok {
			return message{}, false // malformed file message: drop it
		}
		m.file = f
	}
	for _, t := range rumor.Tags {
		if len(t) >= 2 && t[0] == "ms" {
			if v, err := strconv.ParseInt(t[1], 10, 64); err == nil && v >= 0 {
				m.ms = v
			}
		} else if len(t) >= 1 && t[0] == "part" && m.file == nil {
			p, ok := parsePartTag(t)
			if !ok {
				return message{}, false // malformed part tag: drop it
			}
			m.part = &p
		}
	}
	return m, true
}

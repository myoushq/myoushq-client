package myous

// The key a pairing pins: the other side proves it holds the key it sends
// (a signature over a value only the two sides know), and a pairing never
// changes a key that is already a contact: a blocked contact stays
// blocked, an approved one keeps its alias and context. Clients from
// before the proof send none and still pair.

import (
	"crypto/rand"
	"encoding/json"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/nbd-wtf/go-nostr"
	"github.com/nbd-wtf/go-nostr/nip19"
)

type testKeys struct{ sk, pk string }

func newTestKeys() testKeys {
	sk := nostr.GeneratePrivateKey()
	pk, _ := nostr.GetPublicKey(sk)
	return testKeys{sk, pk}
}

// keyFixture is side A (a real pairing) and a mailbox for side B, which a
// test writes out by hand so it can send any payload: what a client from
// before the proof sends, or someone else's key.
type keyFixture struct {
	t   *testing.T
	me  *pairing
	hub *Hub
	key []byte // B's shared key, once finished
}

var contextForErin = ContactContext{Relationship: "friend", Sharing: "my calendar and home address"}

func newKeyFixture(t *testing.T) *keyFixture {
	srv := httptest.NewServer(&mailboxHub{messages: map[string][]string{}})
	t.Cleanup(srv.Close)
	st := &memStorage{docs: map[string][]byte{}}
	st.Put("hub", HubConfig{PairLinkBase: "https://hub.test/p/", URL: srv.URL, FetchedAt: time.Now().Unix()})
	k := newTestKeys()
	return &keyFixture{t: t, me: &pairing{st: st, hub: newHub(st, srv.URL), sk: k.sk, pk: k.pk, alias: "me"}, hub: newHub(st, srv.URL)}
}

// pair invites with cc; side B answers with makePayload(K). Returns A's
// result and the payload A sent.
func (f *keyFixture) pair(cc ContactContext, makePayload func(key []byte) any) (*Pending, payload) {
	t, ctx := f.t, f.t.Context()
	inv, err := f.me.invite(ctx, cc)
	if err != nil {
		t.Fatal(err)
	}
	var claim claimAnswer
	if err := f.hub.Request(ctx, "POST", "/api/pair/"+testNameplate+"/claim", map[string]string{"claim": strings.Repeat("ab", 16)}, "", &claim); err != nil {
		t.Fatal(err)
	}
	post := func(env Envelope) {
		raw, _ := json.Marshal(env)
		if err := f.hub.Request(ctx, "POST", "/api/pair/"+testNameplate+"/messages", map[string]string{"body": b64(raw)}, claim.Token, nil); err != nil {
			t.Fatal(err)
		}
	}
	seed := make([]byte, 32)
	rand.Read(seed)
	post(Envelope{T: "pake", V: 1, M: b64(pakeStart("b", inv.Code, seed))})
	if done, err := f.me.advanceAll(ctx); err != nil || len(done) != 0 { // reads the PAKE, sends A's payload
		t.Fatalf("A: %+v %v", done, err)
	}
	var got struct{ Messages []string }
	if err := f.hub.Request(ctx, "GET", "/api/pair/"+testNameplate+"/messages?after=0&wait=0", nil, claim.Token, &got); err != nil || len(got.Messages) != 2 {
		t.Fatalf("A's messages: %v %v", got, err)
	}
	var envs [2]Envelope
	for i, m := range got.Messages {
		raw, _ := unb64(m)
		json.Unmarshal(raw, &envs[i])
	}
	m, _ := unb64(envs[0].M)
	key, err := pakeFinish("b", inv.Code, seed, m)
	if err != nil {
		t.Fatal(err)
	}
	f.key = key
	plain, _ := json.Marshal(makePayload(key))
	env, err := Seal(key, "b", testNameplate, plain, nil)
	if err != nil {
		t.Fatal(err)
	}
	post(env)
	var mine payload
	if plain, err := Open(key, "a", testNameplate, envs[1]); err != nil || json.Unmarshal(plain, &mine) != nil {
		t.Fatalf("A's payload: %v", err)
	}
	done, err := f.me.advanceAll(ctx)
	if err != nil || len(done) != 1 {
		t.Fatalf("A: %+v %v", done, err)
	}
	return done[0], mine
}

// payloadOf names k's key; signer (nil: no proof, as clients up to 0.7.5)
// signs side B's proof.
func payloadOf(k testKeys, alias string, signer *testKeys) func([]byte) any {
	return func(key []byte) any {
		p := map[string]any{"v": 1, "pubkey": k.pk, "alias": alias}
		if signer != nil {
			sig, _ := signProof(signer.sk, key, "b")
			p["sig"] = sig
		}
		return p
	}
}

func (f *keyFixture) contacts() map[string]Contact {
	c, err := loadContacts(f.me.st)
	if err != nil {
		f.t.Fatal(err)
	}
	return c
}

func (f *keyFixture) last() Entry {
	h, _ := f.me.st.(*memStorage).ReadHistory()
	return h[len(h)-1]
}

func (f *keyFixture) block(k testKeys, alias string, cc ContactContext) {
	if _, err := addContact(f.me.st, k.pk, alias, cc); err != nil {
		f.t.Fatal(err)
	}
	if _, err := updateContact(f.me.st, alias, func(c *Contact) { c.Status = Blocked }); err != nil {
		f.t.Fatal(err)
	}
}

func TestPairingProvesItsKey(t *testing.T) {
	f := newKeyFixture(t)
	bob := newTestKeys()
	r, mine := f.pair(contextForErin, payloadOf(bob, "bob", &bob))
	if r.Stage != "done" || mine.Pubkey != f.me.pk || mine.Sig == nil {
		t.Fatalf("%+v %+v", r, mine)
	}
	if !proofHolds(mine.Pubkey, keyProof(f.key, "a"), *mine.Sig) {
		t.Fatal("A's proof doesn't hold")
	}
	if proofHolds(mine.Pubkey, keyProof(f.key, "b"), *mine.Sig) {
		t.Fatal("a proof is for one side")
	}
	if c := f.contacts()[bob.pk]; c.Status != Approved || c.Relationship != "friend" {
		t.Fatalf("%+v", c)
	}
}

func TestPairingWithoutProofStillPairs(t *testing.T) {
	f := newKeyFixture(t)
	bob := newTestKeys()
	r, _ := f.pair(contextForErin, payloadOf(bob, "bob", nil))
	c := f.contacts()[bob.pk]
	if r.Stage != "done" || r.Known || c.Alias != "bob" || c.Status != Approved || c.Sharing != "my calendar and home address" {
		t.Fatalf("%+v %+v", r, c)
	}
	if f.last().Type != "paired" {
		t.Fatalf("%+v", f.last())
	}
}

// The audit's M2: whoever answers names a blocked contact's key.
func TestPairingBlockedContactStaysBlocked(t *testing.T) {
	for _, proven := range []bool{false, true} {
		f := newKeyFixture(t)
		eve := newTestKeys()
		f.block(eve, "eve", ContactContext{})
		var signer *testKeys
		want := `answered with the key of your contact "eve" (blocked), which the other side didn't prove it holds; ` +
			`nothing changed: if that is right, unblock it (myous unblock "eve")`
		if proven {
			signer, want = &eve, `answered by your contact "eve" (blocked); nothing changed`
		}
		r, _ := f.pair(contextForErin, payloadOf(eve, "Erin", signer))
		all := f.contacts()
		c := all[eve.pk]
		if r.Stage != "failed" || len(all) != 1 || c.Alias != "eve" || c.Status != Blocked || c.Relationship != "" || c.Sharing != "" {
			t.Fatalf("proven=%v: %+v %+v", proven, r, all)
		}
		if e := f.last(); e.Type != "pairing_failed" || !strings.Contains(e.Text, want) {
			t.Fatalf("proven=%v: %+v", proven, e)
		}
	}
}

func TestPairingBadProofFails(t *testing.T) {
	eve, mallory := newTestKeys(), newTestKeys()
	elsewhere, _ := signProof(eve.sk, []byte("another pairing's shared key...."), "b")
	cases := map[string]any{"another key's": nil, "another pairing's": elsewhere, "empty": "", "zeros": strings.Repeat("00", 64),
		"uppercase": nil, "short": "ab"}
	for name, sig := range cases {
		f := newKeyFixture(t)
		r, _ := f.pair(ContactContext{}, func(key []byte) any {
			p := map[string]any{"v": 1, "pubkey": eve.pk, "alias": "Erin", "sig": sig}
			switch name {
			case "another key's":
				p["sig"], _ = signProof(mallory.sk, key, "b")
			case "uppercase":
				s, _ := signProof(eve.sk, key, "b")
				p["sig"] = strings.ToUpper(s)
			}
			return p
		})
		if r.Stage != "failed" || len(f.contacts()) != 0 {
			t.Fatalf("%s: %+v %+v", name, r, f.contacts())
		}
		if name == "another key's" && r.Error != "the other side didn't prove it holds the key it sent" {
			t.Fatalf("%s: %q", name, r.Error)
		}
	}
}

func TestPairingApprovedContactKeepsItsContext(t *testing.T) {
	f := newKeyFixture(t)
	bob := newTestKeys()
	before, err := addContact(f.me.st, bob.pk, "bob", ContactContext{Relationship: "colleague", Sharing: "work only"})
	if err != nil {
		t.Fatal(err)
	}
	r, _ := f.pair(contextForErin, payloadOf(bob, "Erin", nil))
	after := f.contacts()[bob.pk]
	after.PeerKnows = nil // set again: the peer learned our alias
	if r.Stage != "done" || !r.Known || len(f.contacts()) != 1 || after.Alias != before.Alias || after.Status != before.Status ||
		after.Relationship != before.Relationship || after.Sharing != before.Sharing || after.PairedAt != before.PairedAt {
		t.Fatalf("%+v\nbefore %+v\nafter  %+v", r, before, after)
	}
	want := `bob was already your contact: nothing changed (the relationship and sharing given with this invite were not applied; ` +
		`to change them: myous context "bob"`
	if e := f.last(); e.Type != "paired" || !strings.Contains(e.Text, want) {
		t.Fatalf("%+v", e)
	}
}

func TestPairingKeysWrittenOtherwiseAreRefused(t *testing.T) {
	bob := newTestKeys()
	npub, _ := nip19.EncodePublicKey(bob.pk)
	for _, key := range []string{strings.ToUpper(bob.pk), npub, strings.Repeat("ff", 32)} {
		f := newKeyFixture(t)
		r, _ := f.pair(ContactContext{}, func([]byte) any { return map[string]any{"v": 1, "pubkey": key, "alias": "x"} })
		if r.Stage != "failed" || r.Error != "the code didn't match (or the exchange was tampered with)" || len(f.contacts()) != 0 {
			t.Fatalf("%s: %+v", key, r)
		}
	}
	f := newKeyFixture(t)
	r, _ := f.pair(ContactContext{}, func([]byte) any {
		return map[string]any{"v": 1, "pubkey": strings.ToUpper(f.me.pk), "alias": "x"}
	})
	if r.Stage != "failed" || len(f.contacts()) != 0 {
		t.Fatalf("own key in uppercase: %+v", r)
	}
}

// The context goes to the new contact by its key, never to a contact
// whose alias happens to be that key.
func TestPairingAliasNamingAKeyGetsNoContext(t *testing.T) {
	for range 8 { // Go's map order decided which contact findContact found
		f := newKeyFixture(t)
		bob, mallory := newTestKeys(), newTestKeys()
		if _, err := addContact(f.me.st, mallory.pk, bob.pk); err != nil {
			t.Fatal(err)
		}
		r, _ := f.pair(contextForErin, payloadOf(bob, "bob", &bob))
		all := f.contacts()
		if r.Stage != "done" || all[bob.pk].Relationship != "friend" || all[mallory.pk].Relationship != "" {
			t.Fatalf("%+v %+v", r, all)
		}
	}
}

func TestAddContactLeavesAContactAsItIs(t *testing.T) {
	st := &memStorage{docs: map[string][]byte{}}
	eve := newTestKeys()
	addContact(st, eve.pk, "eve", ContactContext{Relationship: "other"})
	updateContact(st, "eve", func(c *Contact) { c.Status = Blocked })
	c, err := addContact(st, eve.pk, "eve again", ContactContext{Relationship: "family"})
	if err != nil || c.Alias != "eve" || c.Status != Blocked || c.Relationship != "other" {
		t.Fatalf("%+v %v", c, err)
	}
}

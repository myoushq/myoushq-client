package myous

import (
	"encoding/json"
	"slices"
	"strings"
	"testing"

	"github.com/nbd-wtf/go-nostr"
)

// cardInbox is an agent with one approved contact ("peer") that sends it
// gift-wrapped text, the way the relay would deliver it.
type cardInbox struct {
	t      *testing.T
	st     *memStorage
	sk     string
	peer   *connection
	peerPK string
}

func newCardInbox(t *testing.T) *cardInbox {
	st := &memStorage{docs: map[string][]byte{}}
	sk := nostr.GeneratePrivateKey()
	peerSK := nostr.GeneratePrivateKey()
	peerPK, _ := nostr.GetPublicKey(peerSK)
	if _, err := addContact(st, peerPK, "peer"); err != nil {
		t.Fatal(err)
	}
	return &cardInbox{t: t, st: st, sk: sk, peer: &connection{sk: peerSK, pk: peerPK}, peerPK: peerPK}
}

// receive delivers one text from the peer and returns the entry stored.
func (ci *cardInbox) receive(text string) Entry {
	ci.t.Helper()
	pk, _ := nostr.GetPublicKey(ci.sk)
	w, err := ci.peer.wrap(pk, kindChat, text, nil)
	if err != nil {
		ci.t.Fatal(err)
	}
	stored, err := handleWraps(ci.st, ci.sk, []*nostr.Event{&w})
	if err != nil || len(stored) != 1 {
		ci.t.Fatalf("stored %d entries: %v", len(stored), err)
	}
	return stored[0]
}

func (ci *cardInbox) contact() Contact {
	contacts, _ := loadContacts(ci.st)
	return contacts[ci.peerPK]
}

func cardJSON(fields map[string]any) string {
	b, _ := json.Marshal(fields)
	return string(b)
}

func TestReceiveCard(t *testing.T) {
	ci := newCardInbox(t)
	about := "Sam's own Mac; the 'myous browser' is its browser"
	e := ci.receive(cardJSON(map[string]any{"myous": "card", "name": "peer", "about": about}))
	if e.Type != "card" || e.Name != "peer" || e.About != about {
		t.Fatalf("got %+v", e)
	}
	if e.Text != "peer describes itself: "+about {
		t.Fatalf("text: %q", e.Text)
	}
	if c := ci.contact(); c.Card == nil || c.Card.About != about || c.Card.Name != "peer" || c.Card.At == 0 {
		t.Fatalf("card not stored: %+v", c.Card)
	}

	// A rename is announced, never applied: the alias is ours.
	renamed := cardJSON(map[string]any{"myous": "card", "name": "Sam's Mac", "about": about})
	e = ci.receive(renamed)
	want := `peer now calls itself "Sam's Mac"; you call it "peer" (keep that, or follow it: myous rename "peer" "Sam's Mac")`
	if e.Text != want {
		t.Fatalf("text: %q\nwant: %q", e.Text, want)
	}
	if c := ci.contact(); c.Alias != "peer" || c.Card.Name != "Sam's Mac" {
		t.Fatalf("alias followed the card: %+v", c)
	}

	// The same card again: noted as unchanged. A cleared description is noted too.
	if e = ci.receive(renamed); e.Text != "peer sent its card again, unchanged" {
		t.Fatalf("text: %q", e.Text)
	}
	e = ci.receive(cardJSON(map[string]any{"myous": "card", "name": "Sam's Mac", "about": ""}))
	if e.Text != "peer cleared its description" || e.About != "" {
		t.Fatalf("got %+v", e)
	}

	// A new name and description at once: both, in that order.
	e = ci.receive(cardJSON(map[string]any{"myous": "card", "name": "Sam", "about": "a Mac"}))
	if !strings.HasPrefix(e.Text, `peer now calls itself "Sam"; `) || !strings.HasSuffix(e.Text, "; describes itself: a Mac") {
		t.Fatalf("text: %q", e.Text)
	}

	// A card without "about" means an empty one (protocol.md section 4).
	e = ci.receive(cardJSON(map[string]any{"myous": "card", "name": "Sam"}))
	if e.Type != "card" || e.Text != "peer cleared its description" {
		t.Fatalf("got %+v", e)
	}
}

func TestUnreadCarriesTheContactsAbout(t *testing.T) {
	ci := newCardInbox(t)
	ci.receive(cardJSON(map[string]any{"myous": "card", "name": "Sam's Mac", "about": "a Mac"}))
	ci.receive("hello")
	entries, err := unread(ci.st, true)
	if err != nil {
		t.Fatal(err)
	}
	last := entries[len(entries)-1]
	if last.Text != "hello" || last.About != "a Mac" {
		t.Fatalf("got %+v", last)
	}
	if entries[0].Type != "card" || entries[0].About != "a Mac" {
		t.Fatalf("card entry: %+v", entries[0])
	}
}

func TestMalformedCardsStayMessages(t *testing.T) {
	ci := newCardInbox(t)
	ci.receive(cardJSON(map[string]any{"myous": "card", "name": "Sam's Mac", "about": "a Mac"}))
	for _, bad := range []map[string]any{
		{"myous": "card", "about": "x"},                                 // no name
		{"myous": "card", "name": strings.Repeat("n", 65), "about": ""}, // long name
		{"myous": "card", "name": "n", "about": strings.Repeat("a", 501)},
		{"myous": "card", "name": 3, "about": ""},  // wrong type
		{"myous": "card", "name": "n", "about": 3}, // wrong type
		{"myous": "card", "name": "   ", "about": ""},
		{"myous": "cards", "name": "n", "about": ""},
	} {
		if e := ci.receive(cardJSON(bad)); e.Type != "message" || e.Text != cardJSON(bad) {
			t.Errorf("%v: got %+v", bad, e)
		}
	}
	if e := ci.receive("not json {"); e.Type != "message" {
		t.Errorf("got %+v", e)
	}
	if c := ci.contact(); c.Card.About != "a Mac" || c.Card.Name != "Sam's Mac" {
		t.Fatalf("a malformed card changed the stored one: %+v", c.Card)
	}
}

func TestParseCardTrimsAndCounts(t *testing.T) {
	name, about, ok := parseCard(`{"myous": "card", "name": "  Sam  ", "about": " ü ", "extra": 1}`)
	if !ok || name != "Sam" || about != "ü" {
		t.Fatalf("got %q %q %v", name, about, ok)
	}
	// Limits count characters, not bytes.
	if _, _, ok := parseCard(cardJSON(map[string]any{"myous": "card", "name": strings.Repeat("ü", 64), "about": strings.Repeat("ü", 500)})); !ok {
		t.Fatal("64-character name and 500-character about refused")
	}
	var obj map[string]any
	if err := json.Unmarshal([]byte(cardText("Max's Muse", "an assistant")), &obj); err != nil ||
		obj["myous"] != "card" || obj["name"] != "Max's Muse" || obj["about"] != "an assistant" {
		t.Fatalf("cardText: %v %v", obj, err)
	}
}

// Which contacts are told this agent's card, and when.
func TestCardsDue(t *testing.T) {
	st := &memStorage{docs: map[string][]byte{}}
	st.Put("settings", map[string]any{"alias": "Max's Muse"})
	agent, err := New(st, "")
	if err != nil {
		t.Fatal(err)
	}
	a, b, c := strings.Repeat("a", 64), strings.Repeat("b", 64), strings.Repeat("c", 64)
	addContact(st, a, "old friend") // from before cards: nothing recorded
	addContact(st, b, "new friend")
	peerKnows(st, b, "Max's Muse", "") // paired now: knows the alias
	addContact(st, c, "blocked one")
	updateContact(st, "blocked one", func(c *Contact) { c.Status = Blocked })
	due := func() string {
		contacts, err := agent.CardsDue()
		if err != nil {
			t.Fatal(err)
		}
		var names []string
		for _, c := range contacts {
			names = append(names, c.Alias)
		}
		slices.Sort(names)
		return strings.Join(names, ",")
	}

	// No card: nothing to say.
	if got := due(); got != "" {
		t.Fatalf("due with no card: %q", got)
	}
	// A card goes to everyone, once.
	if _, err := agent.SetCard("Max's own assistant"); err != nil {
		t.Fatal(err)
	}
	if got := due(); got != "new friend,old friend" {
		t.Fatalf("due after a card: %q", got)
	}
	peerKnows(st, b, "Max's Muse", "Max's own assistant")
	if got := due(); got != "old friend" {
		t.Fatalf("due after telling one: %q", got)
	}
	// A rename is announced to those who knew the old name; the old friend
	// never recorded a name, so there is nothing to correct.
	agent.SetCard("")
	st.Put("settings", map[string]any{"alias": "Max's Assistant"})
	if got := due(); got != "new friend" {
		t.Fatalf("due after a rename: %q", got)
	}

	// Limits.
	if _, err := agent.SetCard(strings.Repeat("x", 501)); err == nil {
		t.Fatal("over-long card accepted")
	}
	if about, err := agent.SetCard("  spaced  "); err != nil || about != "spaced" || agent.Card() != "spaced" {
		t.Fatalf("got %q %v", about, err)
	}
	if about, _ := agent.SetCard(""); about != "" || agent.Card() != "" {
		t.Fatal("card not cleared")
	}
}

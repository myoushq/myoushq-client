package myous

import (
	"context"
	"fmt"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/nbd-wtf/go-nostr"
)

// countingStorage counts the writes of each document.
type countingStorage struct {
	*FileStorage
	mu   sync.Mutex
	puts map[string]int
}

func (c *countingStorage) Put(name string, v any) error {
	c.mu.Lock()
	c.puts[name]++
	c.mu.Unlock()
	return c.FileStorage.Put(name, v)
}

// A wrap seen before (a relay sends everything it holds on every connect)
// costs no write of the state.
func TestHandleWrapsWritesTheStateOnlyWhenItChanged(t *testing.T) {
	fs, err := NewFileStorage(filepath.Join(t.TempDir(), "h"))
	if err != nil {
		t.Fatal(err)
	}
	st := &countingStorage{FileStorage: fs, puts: map[string]int{}}
	sk := nostr.GeneratePrivateKey()
	now := time.Now().Unix()
	st.FileStorage.Put("state", State{NextSeq: 1, Seen: map[string]int64{"aa": now, "bb": now}})
	if _, err := handleWraps(st, sk, []*nostr.Event{{ID: "aa"}, {ID: "bb"}}); err != nil {
		t.Fatal(err)
	}
	if st.puts["state"] != 0 {
		t.Fatalf("state written %d times for wraps seen before", st.puts["state"])
	}
	// A new one (even one that doesn't open) is remembered.
	if _, err := handleWraps(st, sk, []*nostr.Event{{ID: "aa"}, {ID: "cc"}}); err != nil {
		t.Fatal(err)
	}
	s, _ := loadState(st)
	if st.puts["state"] != 1 || s.Seen["cc"] == 0 {
		t.Fatalf("%d writes; seen %v", st.puts["state"], s.Seen)
	}
	// One past its retention is forgotten, which is a change too.
	st.FileStorage.Put("state", State{NextSeq: 1, Seen: map[string]int64{"aa": now, "old": now - seenRetention}})
	handleWraps(st, sk, []*nostr.Event{{ID: "aa"}})
	if s, _ := loadState(st); st.puts["state"] != 2 || len(s.Seen) != 1 {
		t.Fatalf("%d writes; seen %v", st.puts["state"], s.Seen)
	}
}

// Messages wrapped back to back, several within a millisecond, come out
// of handleWraps in the order written whatever order the relay gives:
// their (created_at, ms) never tie. Before, two in the same millisecond
// kept the relay's order, which is random.
func TestMessagesWrappedWithinAMillisecondKeepTheirOrder(t *testing.T) {
	fs, err := NewFileStorage(filepath.Join(t.TempDir(), "h"))
	if err != nil {
		t.Fatal(err)
	}
	sk := nostr.GeneratePrivateKey()
	pk, _ := nostr.GetPublicKey(sk)
	const n = 300
	wraps, sender := wrapsFrom(t, pk, n)
	if _, err := addContact(fs, sender, "alice"); err != nil {
		t.Fatal(err)
	}
	var prev message
	for i, w := range wraps {
		m, ok := unwrap(sk, w)
		if !ok {
			t.Fatalf("wrap %d doesn't open", i)
		}
		if m.sentAt != m.ms/1000 || (i > 0 && m.ms <= prev.ms) {
			t.Fatalf("wrap %d: created_at %d, ms %d after %d", i, m.sentAt, m.ms, prev.ms)
		}
		prev = m
	}
	slices.Reverse(wraps) // the opposite order: a tie would come out reversed
	got, err := handleWraps(fs, sk, wraps)
	if err != nil {
		t.Fatal(err)
	}
	var want, texts []string
	for i, e := range got {
		want = append(want, fmt.Sprintf("message %03d", i))
		texts = append(texts, e.Text)
	}
	if len(got) != n || strings.Join(texts, "|") != strings.Join(want, "|") {
		t.Fatalf("%d of %d, %s", len(got), n, firstMismatch(want, texts))
	}
}

// listenInTest runs a.Listen until the test ends, then cancels it and
// waits for it to return: a Listen left running closes its connection
// during a later test, where go-nostr's known close race is reported.
func listenInTest(t *testing.T, a *Agent, onNew func([]Entry)) {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() { done <- a.Listen(ctx, onNew, nil, 20*time.Second) }()
	t.Cleanup(func() {
		cancel()
		select {
		case <-done:
		case <-time.After(20 * time.Second):
			t.Error("Listen didn't return after its context ended")
		}
	})
}

// Messages sent while an agent wasn't listening come out of Listen in the
// order they were written (the relay sends them in the random order of
// the wraps' timestamps); a worker runs such a backlog in order.
func TestListenHandsOverABacklogInOrder(t *testing.T) {
	e := setup(t)
	e.init("go-ord1", "go-ord2")
	e.pair("go-ord1", "go-ord2", "code")
	const n = 12
	var want []string
	for i := 0; i < n; i++ {
		text := fmt.Sprintf("backlog %02d", i)
		e.ok("go-ord1", "send", "go-ord2", text)
		want = append(want, text)
	}
	st, err := NewFileStorage(filepath.Join(e.dir, "go-ord2"))
	if err != nil {
		t.Fatal(err)
	}
	agent, err := New(st, "")
	if err != nil {
		t.Fatal(err)
	}
	var mu sync.Mutex
	var got []string
	all := make(chan struct{})
	listenInTest(t, agent, func(entries []Entry) {
		mu.Lock()
		defer mu.Unlock()
		for _, x := range entries {
			if x.Type == "message" {
				got = append(got, x.Text)
			}
		}
		if len(got) == n {
			close(all)
		}
	})
	select {
	case <-all:
	case <-time.After(30 * time.Second):
		mu.Lock()
		defer mu.Unlock()
		t.Fatalf("got %d of %d: %v", len(got), n, got)
	}
	if strings.Join(got, "|") != strings.Join(want, "|") {
		t.Fatalf("out of order:\n%v", got)
	}
}

// BenchmarkHandleWrapsSeen: one wrap already seen, in a state that
// remembers 500.
func BenchmarkHandleWrapsSeen(b *testing.B) {
	st, err := NewFileStorage(filepath.Join(b.TempDir(), "h"))
	if err != nil {
		b.Fatal(err)
	}
	s := State{NextSeq: 1, Seen: map[string]int64{}}
	now := time.Now().Unix()
	var ids []string
	for i := 0; i < 500; i++ {
		id := fmt.Sprintf("%064x", i)
		s.Seen[id] = now
		ids = append(ids, id)
	}
	st.Put("state", s)
	sk := nostr.GeneratePrivateKey()
	b.ReportAllocs()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		if _, err := handleWraps(st, sk, []*nostr.Event{{ID: ids[i%len(ids)]}}); err != nil {
			b.Fatal(err)
		}
	}
}

// firstMismatch says where got first differs from want, with both values.
func firstMismatch(want, got []string) string {
	for i := range min(len(want), len(got)) {
		if want[i] != got[i] {
			return fmt.Sprintf("first difference at %d: sent %q, got %q", i, want[i], got[i])
		}
	}
	return fmt.Sprintf("the first %d agree", min(len(want), len(got)))
}

// More wraps than the hub answers a query or a subscription without a
// limit (250), fewer than a page (wrapPage): Poll and a listener's
// catch-up fetch get every one, as one batch in the order written. Before,
// a listener's replay of its subscription stopped at 250, in the random
// order of the wraps' timestamps.
func TestListenAndPollGetAPageOfStoredWraps(t *testing.T) {
	e := setup(t)
	e.init("go-many", "go-poller", "go-listener")
	e.pair("go-many", "go-poller", "code")
	e.pair("go-many", "go-listener", "code")
	ctx := context.Background()
	st, _ := NewFileStorage(filepath.Join(e.dir, "go-many"))
	sender, _ := New(st, "")
	conn, err := sender.connect(ctx, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer conn.close()
	// Straight onto the relay: no history to write for each one.
	sendN := func(to string, n int) []string {
		t.Helper()
		pk, _, err := findContact(st, to)
		if err != nil {
			t.Fatal(err)
		}
		var texts []string
		for i := 0; i < n; i++ {
			text := fmt.Sprintf("%s %03d", to, i)
			if _, err := conn.sendMessage(ctx, pk, text, nil, []string{conn.relays[0].URL}); err != nil {
				t.Fatal(err)
			}
			texts = append(texts, text)
		}
		return texts
	}
	texts := func(entries []Entry) []string {
		var s []string
		for _, x := range entries {
			if x.Type == "message" {
				s = append(s, x.Text)
			}
		}
		return s
	}

	const many = 270
	want := sendN("go-poller", many)
	pst, _ := NewFileStorage(filepath.Join(e.dir, "go-poller"))
	poller, _ := New(pst, "")
	polled := time.Now()
	got, err := poller.Poll(ctx)
	if err != nil {
		t.Fatal(err)
	}
	t.Logf("poll of %d took %v", len(want), time.Since(polled))
	if g := texts(got); strings.Join(g, "|") != strings.Join(want, "|") {
		t.Fatalf("poll: %d of %d, %s", len(g), len(want), firstMismatch(want, g))
	}

	want = sendN("go-listener", many)
	lst, _ := NewFileStorage(filepath.Join(e.dir, "go-listener"))
	listener, _ := New(lst, "")
	var mu sync.Mutex
	var heard []string
	all := make(chan struct{})
	listenInTest(t, listener, func(entries []Entry) {
		mu.Lock()
		defer mu.Unlock()
		heard = append(heard, texts(entries)...)
		if len(heard) == len(want) {
			close(all)
		}
	})
	select {
	case <-all:
	case <-time.After(60 * time.Second):
		mu.Lock()
		defer mu.Unlock()
		t.Fatalf("listen: %d of %d", len(heard), len(want))
	}
	mu.Lock()
	defer mu.Unlock()
	if strings.Join(heard, "|") != strings.Join(want, "|") {
		t.Fatalf("listen: out of order, %s", firstMismatch(want, heard))
	}
	// A fetch that gets no answer is an error, not an empty inbox.
	gone, cancelGone := context.WithCancel(ctx)
	cancelGone()
	if _, err := conn.fetchWraps(gone); err == nil {
		t.Fatal("no error from a fetch that got no answer")
	}
	// And what comes next, live.
	heard = nil
	mu.Unlock()
	sendN("go-listener", 1)
	time.Sleep(2 * time.Second)
	mu.Lock()
	if len(heard) != 1 {
		t.Fatalf("live after the backlog: %v", heard)
	}
}

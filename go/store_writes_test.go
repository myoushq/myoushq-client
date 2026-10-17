package myous

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"

	"github.com/nbd-wtf/go-nostr"
)

// writeCounter counts a storage's writes.
type writeCounter struct {
	*FileStorage
	mu      sync.Mutex
	puts    map[string]int
	appends int // calls of AppendHistory
	reads   int // calls of ReadHistory
}

func (c *writeCounter) Put(name string, v any) error {
	c.mu.Lock()
	c.puts[name]++
	c.mu.Unlock()
	return c.FileStorage.Put(name, v)
}

func (c *writeCounter) AppendHistory(e Entry) error {
	c.appends++
	return c.FileStorage.AppendHistory(e)
}

func (c *writeCounter) ReadHistory() ([]Entry, error) {
	c.reads++
	return c.FileStorage.ReadHistory()
}

// wrapsFrom are n gift wraps of text messages from a new sender to
// recipient, and the sender's public key.
func wrapsFrom(t testing.TB, recipient string, n int) ([]*nostr.Event, string) {
	t.Helper()
	sk := nostr.GeneratePrivateKey()
	pk, _ := nostr.GetPublicKey(sk)
	c := &connection{sk: sk, pk: pk}
	var wraps []*nostr.Event
	for i := 0; i < n; i++ {
		w, err := c.wrap(recipient, kindChat, fmt.Sprintf("message %03d", i), nil)
		if err != nil {
			t.Fatal(err)
		}
		wraps = append(wraps, &w)
	}
	return wraps, pk
}

// A batch of messages is stored with one state update (plus the one that
// marks the wraps handled), numbered in order; a FileStorage appends it
// with one write (appendHistory), and a storage that embeds one and
// overrides AppendHistory has its own method called for each entry.
func TestHandleWrapsStoresABatchAtOnce(t *testing.T) {
	fs, err := NewFileStorage(filepath.Join(t.TempDir(), "h"))
	if err != nil {
		t.Fatal(err)
	}
	st := &writeCounter{FileStorage: fs, puts: map[string]int{}}
	sk := nostr.GeneratePrivateKey()
	pk, _ := nostr.GetPublicKey(sk)
	wraps, sender := wrapsFrom(t, pk, 30)
	if _, err := addContact(fs, sender, "alice"); err != nil {
		t.Fatal(err)
	}
	record(fs, Entry{Type: "message", Direction: "out", Text: "before"})
	got, err := handleWraps(st, sk, wraps)
	if err != nil {
		t.Fatal(err)
	}
	if len(got) != 30 || st.appends != 30 || st.puts["state"] != 2 {
		t.Fatalf("%d stored with %d appends and %d state writes", len(got), st.appends, st.puts["state"])
	}
	history, _ := fs.ReadHistory()
	for i, e := range history[1:] {
		if e.Seq != i+2 || e.Text != fmt.Sprintf("message %03d", i) || got[i].Seq != e.Seq || e.Alias != "alice" {
			t.Fatalf("entry %d: %+v (returned %+v)", i, e, got[i])
		}
	}
	if s, _ := loadState(fs); s.NextSeq != 32 {
		t.Fatalf("next seq %d", s.NextSeq)
	}
	// The same wraps again: nothing written at all.
	st.appends, st.puts = 0, map[string]int{}
	if got, err := handleWraps(st, sk, wraps); err != nil || len(got) != 0 || st.appends != 0 || len(st.puts) != 0 {
		t.Fatalf("seen again: %d stored, %d appends, %v", len(got), st.appends, st.puts)
	}
}

// Processes writing the same document at once (the hub's cached config)
// each get their write through, and a reader never sees half of one: each
// write has a temporary file of its own.
func TestConcurrentPutsOfOneDocument(t *testing.T) {
	st, err := NewFileStorage(filepath.Join(t.TempDir(), "h"))
	if err != nil {
		t.Fatal(err)
	}
	doc := map[string]string{"relays": strings.Repeat("wss://relay.example/ ", 2000)}
	var wg sync.WaitGroup
	var mu sync.Mutex
	var failures []string
	fail := func(s string) {
		mu.Lock()
		failures = append(failures, s)
		mu.Unlock()
	}
	for w := 0; w < 4; w++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for i := 0; i < 50; i++ {
				if err := st.Put("hub", doc); err != nil {
					fail("put: " + err.Error())
				}
			}
		}()
	}
	done := make(chan struct{})
	go func() {
		defer close(done)
		for i := 0; i < 200; i++ {
			raw, err := os.ReadFile(st.Path("hub.json"))
			if err == nil && json.Unmarshal(raw, &map[string]string{}) != nil {
				fail("read a torn document")
			}
		}
	}()
	wg.Wait()
	<-done
	if len(failures) > 0 {
		t.Fatalf("%d failures, first: %s", len(failures), failures[0])
	}
	if left, _ := filepath.Glob(st.Path("hub.json.*")); len(left) != 0 {
		t.Fatalf("temporary files left: %v", left)
	}
}

// BenchmarkHandleWraps200: a backlog of 200 new messages, stored.
func BenchmarkHandleWraps200(b *testing.B) {
	sk := nostr.GeneratePrivateKey()
	pk, _ := nostr.GetPublicKey(sk)
	wraps, sender := wrapsFrom(b, pk, 200)
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		b.StopTimer()
		st, _ := NewFileStorage(filepath.Join(b.TempDir(), fmt.Sprint(i)))
		addContact(st, sender, "alice")
		b.StartTimer()
		if got, err := handleWraps(st, sk, wraps); err != nil || len(got) != 200 {
			b.Fatal(len(got), err)
		}
	}
}

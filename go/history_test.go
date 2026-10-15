package myous

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

// ReadHistorySince is ReadHistory filtered, for every seq, on a history
// with what both clients write: Go's lines, Python's (sorted keys, CRLF
// on Windows), blank lines, a last line without its newline, and lines
// longer than what it reads at a time.
func TestReadHistorySinceIsReadHistoryFiltered(t *testing.T) {
	defer func(n int) { historyChunk = n }(historyChunk)
	historyChunk = 64 // small, so lines span chunks
	st, err := NewFileStorage(filepath.Join(t.TempDir(), "h"))
	if err != nil {
		t.Fatal(err)
	}
	long := strings.Repeat("ünïcödé ", 100)
	lines := []string{
		`{"seq":1,"type":"message","direction":"in","peer":"npub1a","text":"hi","at":1}`,
		``,
		`{"alias": "py", "at": 2, "direction": "in", "peer": "npub1b", "seq": 2, "text": "from Python", "type": "message"}` + "\r",
		`{"seq":3,"type":"message","direction":"out","peer":"npub1a","text":"` + long + `","at":3}`,
		`   `,
		`{"seq":4,"type":"paired","peer":"npub1c","text":"paired","at":4,"verify":"12345"}`,
		`{"alias": "py", "at": 5, "direction": "in", "id": "abab", "peer": "npub1b", "seq": 5, "text": "{}", "type": "result"}` + "\r",
		`{"seq":6,"type":"message","direction":"in","peer":"npub1a","text":"` + long + long + `","at":6}`,
	}
	for _, last := range []string{"\n", ""} { // with and without the last newline
		if err := os.WriteFile(st.Path("messages.jsonl"), []byte(strings.Join(lines, "\n")+last), 0o600); err != nil {
			t.Fatal(err)
		}
		all, err := st.ReadHistory()
		if err != nil || len(all) != 6 {
			t.Fatalf("ReadHistory: %d, %v", len(all), err)
		}
		for seq := 0; seq <= 8; seq++ {
			var want []Entry
			for _, e := range all {
				if e.Seq >= seq {
					want = append(want, e)
				}
			}
			got, err := st.ReadHistorySince(seq)
			if err != nil {
				t.Fatal(err)
			}
			if !reflect.DeepEqual(got, want) {
				t.Fatalf("since %d (last %q): got %+v\nwant %+v", seq, last, got, want)
			}
		}
	}
	// No history yet: nothing, no error.
	os.Remove(st.Path("messages.jsonl"))
	if got, err := st.ReadHistorySince(1); err != nil || len(got) != 0 {
		t.Fatalf("no file: %v, %v", got, err)
	}
	// A line that doesn't decode is an error, as for ReadHistory, when it
	// is among the lines asked for.
	os.WriteFile(st.Path("messages.jsonl"), []byte(lines[0]+"\nnot json\n"+lines[5]+"\n"), 0o600)
	if _, err := st.ReadHistorySince(1); err == nil {
		t.Fatal("no error for a bad line")
	}
	if got, err := st.ReadHistorySince(5); err != nil || len(got) != 0 {
		t.Fatalf("since 5: %v, %v", got, err)
	}
}

// What is new reads the same through ReadHistorySince and through a
// storage that only has ReadHistory.
func TestEntriesSinceAndUnreadWithAndWithoutTail(t *testing.T) {
	fs, err := NewFileStorage(filepath.Join(t.TempDir(), "h"))
	if err != nil {
		t.Fatal(err)
	}
	mem := &memStorage{docs: map[string][]byte{}}
	for _, st := range []Storage{fs, mem} {
		for i := 1; i <= 6; i++ {
			e := Entry{Type: "message", Direction: "in", Peer: "npub1a", Text: fmt.Sprint(i)}
			if i%3 == 0 {
				e.Direction = "out"
			}
			if i == 4 {
				e.Type, e.ID = "result", "abab" // taken by a waiting exec: not unread
			}
			if _, err := record(st, e); err != nil {
				t.Fatal(err)
			}
		}
	}
	texts := func(es []Entry) string {
		var s []string
		for _, e := range es {
			s = append(s, e.Text)
		}
		return strings.Join(s, ",")
	}
	for _, st := range []Storage{fs, mem} {
		a := &Agent{st: st}
		since, err := a.entriesSince(2)
		if err != nil || texts(since) != "2,4,5" {
			t.Fatalf("%T entriesSince: %q, %v", st, texts(since), err)
		}
		got, err := unread(st, true)
		if err != nil || texts(got) != "1,2,5" {
			t.Fatalf("%T unread: %q, %v", st, texts(got), err)
		}
		record(st, Entry{Type: "message", Direction: "in", Peer: "npub1a", Text: "7"})
		if got, err := unread(st, true); err != nil || texts(got) != "7" {
			t.Fatalf("%T unread again: %q, %v", st, texts(got), err)
		}
	}
}

// BenchmarkEntriesSince: what is new, in a worker's history of 2000
// requests with 4 KB of output each (about 10 MB).
func BenchmarkEntriesSince(b *testing.B) {
	st, err := NewFileStorage(filepath.Join(b.TempDir(), "h"))
	if err != nil {
		b.Fatal(err)
	}
	f, err := os.Create(st.Path("messages.jsonl"))
	if err != nil {
		b.Fatal(err)
	}
	out := strings.Repeat("x", 4000)
	seq := 1
	for i := 0; i < 2000; i++ {
		for _, e := range []Entry{
			{Seq: seq, Type: "message", Direction: "in", Peer: "npub1a", Text: fmt.Sprintf(`{"myous": "exec", "id": "%032d", "cmd": "make"}`, i)},
			{Seq: seq + 1, Type: "message", Direction: "out", Peer: "npub1a", Text: `{"exit": 0, "stdout": "` + out + `"}`},
		} {
			raw, _ := json.Marshal(e)
			f.Write(append(raw, '\n'))
		}
		seq += 2
	}
	f.Close()
	st.Put("state", State{NextSeq: seq + 1})
	record(st, Entry{Type: "message", Direction: "in", Peer: "npub1a", Text: "new"})
	a := &Agent{st: st}
	b.ReportAllocs()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		if es, err := a.entriesSince(seq + 1); err != nil || len(es) != 1 {
			b.Fatal(len(es), err)
		}
	}
}

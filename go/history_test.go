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

// readHistorySince is ReadHistory filtered, for every seq, on a history
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
			got, err := st.readHistorySince(seq)
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
	if got, err := st.readHistorySince(1); err != nil || len(got) != 0 {
		t.Fatalf("no file: %v, %v", got, err)
	}
	// A line that doesn't decode is an error, as for ReadHistory, when it
	// is among the lines asked for.
	os.WriteFile(st.Path("messages.jsonl"), []byte(lines[0]+"\nnot json\n"+lines[5]+"\n"), 0o600)
	if _, err := st.readHistorySince(1); err == nil {
		t.Fatal("no error for a bad line")
	}
	if got, err := st.readHistorySince(5); err != nil || len(got) != 0 {
		t.Fatalf("since 5: %v, %v", got, err)
	}
}

// A storage that embeds FileStorage and overrides ReadHistory or
// AppendHistory has its own methods called, not FileStorage's faster
// ones promoted past them.
func TestAnEmbeddingStorageKeepsItsOwnHistoryMethods(t *testing.T) {
	fs, err := NewFileStorage(filepath.Join(t.TempDir(), "h"))
	if err != nil {
		t.Fatal(err)
	}
	st := &writeCounter{FileStorage: fs, puts: map[string]int{}}
	if _, err := recordMany(st, []Entry{{Text: "a"}, {Text: "b"}}); err != nil {
		t.Fatal(err)
	}
	got, err := historySince(st, 2)
	if err != nil || len(got) != 1 || got[0].Text != "b" {
		t.Fatalf("since 2: %+v, %v", got, err)
	}
	if st.appends != 2 || st.reads != 1 {
		t.Fatalf("AppendHistory called %d times, ReadHistory %d", st.appends, st.reads)
	}
}

// What is new reads the same through readHistorySince and through a
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

// A write cut short (a crash, a full disk) leaves a fragment after the
// last newline: both readers go on without it, and the next append drops
// it (or ends the line, when it is a whole entry), so the new entry gets
// a line of its own. Until then one torn write failed every read, which
// a listener ignores: a worker stopped handling requests, silently.
func TestHistoryTornTail(t *testing.T) {
	defer func(n int) { historyChunk = n }(historyChunk)
	historyChunk = 16 // the mend looks back across several blocks
	st, err := NewFileStorage(filepath.Join(t.TempDir(), "h"))
	if err != nil {
		t.Fatal(err)
	}
	path := st.Path("messages.jsonl")
	whole := `{"seq":1,"type":"message","direction":"in","peer":"npub1a","text":"one","at":1}` + "\n" +
		`{"seq":2,"type":"message","direction":"in","peer":"npub1a","text":"two","at":2}` + "\n"
	texts := func() string {
		t.Helper()
		all, err := st.ReadHistory()
		if err != nil {
			t.Fatal(err)
		}
		since, err := st.readHistorySince(1)
		if err != nil {
			t.Fatal(err)
		}
		if !reflect.DeepEqual(all, since) {
			t.Fatalf("ReadHistory %+v, readHistorySince %+v", all, since)
		}
		var s []string
		for _, e := range all {
			s = append(s, e.Text)
		}
		return strings.Join(s, ",")
	}
	for _, torn := range []string{`{"seq":3,"type":"mess`, `{"seq":3,"type":"message","text":"long line ` + strings.Repeat("x", 100), `[1, 2`} {
		os.WriteFile(path, []byte(whole+torn), 0o600)
		if got := texts(); got != "one,two" {
			t.Fatalf("torn %q: read %q", torn, got)
		}
		if err := st.AppendHistory(Entry{Seq: 3, Type: "message", Direction: "in", Text: "three"}); err != nil {
			t.Fatal(err)
		}
		if got := texts(); got != "one,two,three" {
			t.Fatalf("after the append: %q", got)
		}
		if raw, _ := os.ReadFile(path); !strings.HasPrefix(string(raw), whole+`{"seq":3,"type":"message","direction":"in","text":"three"`) {
			t.Fatalf("not mended:\n%s", raw)
		}
	}
	// A whole entry without its newline (another writer's last line) is
	// kept: the append ends its line first.
	os.WriteFile(path, []byte(strings.TrimSuffix(whole, "\n")), 0o600)
	st.AppendHistory(Entry{Seq: 3, Type: "message", Text: "three"})
	if got := texts(); got != "one,two,three" {
		t.Fatalf("whole last line: %q", got)
	}
	// The file that is only a fragment, and the empty file.
	os.WriteFile(path, []byte(`{"seq":1,"ty`), 0o600)
	if got := texts(); got != "" {
		t.Fatalf("only a fragment: %q", got)
	}
	st.AppendHistory(Entry{Seq: 1, Type: "message", Text: "one"})
	if got := texts(); got != "one" {
		t.Fatalf("after mending the only line: %q", got)
	}
	os.WriteFile(path, nil, 0o600)
	st.AppendHistory(Entry{Seq: 1, Type: "message", Text: "one"})
	if got := texts(); got != "one" {
		t.Fatalf("empty file: %q", got)
	}
	// A bad line that isn't the last is still an error.
	os.WriteFile(path, []byte(`{"seq":1,"ty`+"\n"+strings.SplitN(whole, "\n", 2)[1]), 0o600)
	if _, err := st.ReadHistory(); err == nil {
		t.Fatal("no error for a bad line in the middle")
	}
}

package main

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	myous "github.com/myoushq/myoushq-client/go"
)

func TestLastUsed(t *testing.T) {
	dir := t.TempDir()
	if got := lastUsed(dir); got != 0 {
		t.Fatalf("empty directory: %d", got)
	}
	old := time.Now().Add(-time.Hour).Truncate(time.Second)
	for _, tc := range []struct {
		file string
		at   time.Time
		want int64
	}{
		{"settings.json", old, old.Unix()},
		{"venv/bin/python", time.Now(), old.Unix()}, // installs don't count
		{"bin/myous", time.Now(), old.Unix()},
		{"locks/state", time.Now(), old.Unix()},
		{"history/x.jsonl", old.Add(time.Minute), old.Unix() + 60},
	} {
		path := filepath.Join(dir, filepath.FromSlash(tc.file))
		os.MkdirAll(filepath.Dir(path), 0o700)
		os.WriteFile(path, []byte("{}"), 0o600)
		os.Chtimes(path, tc.at, tc.at)
		if got := lastUsed(dir); got != tc.want {
			t.Fatalf("after %s: lastUsed = %d, want %d", tc.file, got, tc.want)
		}
	}
}

func TestCardAndContextLines(t *testing.T) {
	c := myous.Contact{Alias: "peer"}
	if got := cardLine(c); got != "" {
		t.Fatalf("no card: %q", got)
	}
	c.Card = &myous.Card{Name: "peer", About: ""}
	if got := cardLine(c); got != "" {
		t.Fatalf("same name, no about: %q", got)
	}
	c.Card = &myous.Card{Name: "peer", About: "a Mac"}
	if got := cardLine(c); got != "    about: a Mac" {
		t.Fatalf("got %q", got)
	}
	c.Card = &myous.Card{Name: "Sam's Mac", About: ""}
	if got := cardLine(c); got != `    calls itself "Sam's Mac"; about: (none)` {
		t.Fatalf("got %q", got)
	}

	e := myous.Entry{Alias: "peer", Relationship: "friend", Sharing: "the weather", About: "a Mac"}
	if got := contextLine(e); got != "    (friend; may share: the weather; it says of itself: a Mac)" {
		t.Fatalf("got %q", got)
	}
	e.Sharing = ""
	if got := contextLine(e); got != "    (friend; it says of itself: a Mac)" {
		t.Fatalf("got %q", got)
	}
}

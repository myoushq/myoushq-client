package main

import (
	"os"
	"path/filepath"
	"testing"
	"time"
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

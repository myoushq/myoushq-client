//go:build !windows

package myous

import (
	"os"
	"path/filepath"
	"syscall"
	"testing"
)

// mkdir's mode only applies to a directory it creates: an existing myous
// directory (made by `python3 -m venv` or `go build -o "$MYOUS_HOME/bin/..."`
// before the first command, or by hand) is usually 0755, and the files
// written in it without a mode of their own (worker.log, request records)
// would be readable by every user.
func TestExistingHomeBecomesPrivate(t *testing.T) {
	old := syscall.Umask(0o022)
	defer syscall.Umask(old)
	home := filepath.Join(t.TempDir(), "home")
	if err := os.Mkdir(home, 0o755); err != nil {
		t.Fatal(err)
	}
	if _, err := NewFileStorage(home); err != nil {
		t.Fatal(err)
	}
	fi, err := os.Stat(home)
	if err != nil {
		t.Fatal(err)
	}
	if got := fi.Mode().Perm(); got != 0o700 {
		t.Fatalf("home mode %o, want 700", got)
	}
}

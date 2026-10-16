//go:build !windows

package myous

import (
	"os"
	"syscall"
)

// makePrivate makes a directory of this user's readable by this user
// alone. MkdirAll's mode only applies to a directory it creates: one made
// first by `python3 -m venv`, `go build -o` or by hand is usually 0755, and
// the files written in it without a mode of their own (worker.log, request
// records) would be readable by every user. A directory owned by someone
// else is left alone.
func makePrivate(dir string) {
	fi, err := os.Stat(dir)
	if err != nil || fi.Mode().Perm()&0o077 == 0 {
		return
	}
	if st, ok := fi.Sys().(*syscall.Stat_t); ok && int(st.Uid) == os.Getuid() {
		_ = os.Chmod(dir, fi.Mode().Perm()&0o700)
	}
}

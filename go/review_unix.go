//go:build !windows

package myous

import (
	"os"
	"syscall"
)

// openSetting opens a setting file (review.json) for reading without
// following a link and without waiting on a pipe (review.py's
// _read_regular does the same).
func openSetting(path string) (*os.File, error) {
	return os.OpenFile(path, os.O_RDONLY|syscall.O_NOFOLLOW|syscall.O_NONBLOCK, 0)
}

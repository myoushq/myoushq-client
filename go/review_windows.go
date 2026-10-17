//go:build windows

package myous

import "os"

// openSetting opens a setting file (review.json) for reading. Windows has
// no O_NOFOLLOW or FIFOs in a folder; the caller has checked with Lstat
// that it is a regular file, and checks the open file again.
func openSetting(path string) (*os.File, error) {
	return os.Open(path)
}

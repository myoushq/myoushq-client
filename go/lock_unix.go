//go:build !windows

package myous

import (
	"errors"
	"os"
	"syscall"
)

func lockFile(f *os.File, wait bool) (bool, error) {
	how := syscall.LOCK_EX
	if !wait {
		how |= syscall.LOCK_NB
	}
	if err := syscall.Flock(int(f.Fd()), how); err != nil {
		if errors.Is(err, syscall.EWOULDBLOCK) {
			return false, nil
		}
		return false, err
	}
	return true, nil
}

func unlockFile(f *os.File) { syscall.Flock(int(f.Fd()), syscall.LOCK_UN) }

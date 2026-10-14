//go:build !windows

package myous

import (
	"context"
	"os"
	"os/exec"
	"strings"
	"syscall"

	"golang.org/x/sys/unix"
)

const isWindows = false

// shellCommand runs cmd with /bin/sh in a new session (and so a process
// group of its own), so killTree can end what it starts too.
func shellCommand(cmd string) *exec.Cmd {
	c := exec.Command("/bin/sh", "-c", cmd)
	c.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	return c
}

// killTree ends a command started by shellCommand and everything in its
// process group.
func killTree(c *exec.Cmd) {
	syscall.Kill(-c.Process.Pid, syscall.SIGKILL)
}

// hookCommand runs a review command as Python's shell=True with
// start_new_session does: in a session of its own, so that ending it
// (the worker stopping, or the hook's time limit) ends what the hook
// started too. Without this, killing the shell alone leaves a child
// holding the hook's output pipe, and the worker waits for it.
func hookCommand(ctx context.Context, cmd string) *exec.Cmd {
	c := exec.CommandContext(ctx, "/bin/sh", "-c", cmd)
	c.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	c.Cancel = func() error { return syscall.Kill(-c.Process.Pid, syscall.SIGKILL) }
	return c
}

// exitCode is the status as Python reports it: the exit code, or minus
// the signal that ended the process.
func exitCode(ps *os.ProcessState) int {
	if ws, ok := ps.Sys().(syscall.WaitStatus); ok && ws.Signaled() {
		return -int(ws.Signal())
	}
	return ps.ExitCode()
}

// removeEarly removes a temporary file while it is open: it lives on
// until closed, and nothing is left behind.
func removeEarly(f *os.File) { os.Remove(f.Name()) }

// machine is Python's platform.machine(): the hardware name uname gives.
func machine() string {
	var u unix.Utsname
	if unix.Uname(&u) != nil {
		return ""
	}
	return strings.TrimRight(string(u.Machine[:]), "\x00")
}

// hideWindow keeps a helper program from opening a console window (only
// Windows has them).
func hideWindow(c *exec.Cmd) {}

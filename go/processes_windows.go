//go:build windows

package myous

import (
	"context"
	"encoding/base64"
	"encoding/binary"
	"fmt"
	"os"
	"os/exec"
	"strconv"
	"syscall"
	"unicode/utf16"

	"golang.org/x/sys/windows"
)

const isWindows = true

// Before the owner's command: no progress bars on stderr, UTF-8 output.
// After it: the exit status of what failed, so `exit` codes and failing
// programs come back as they would from a POSIX shell. (As processes.py.)
const (
	psBefore = "$ProgressPreference = 'SilentlyContinue'\n" +
		"[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)\n" +
		"$OutputEncoding = [Console]::OutputEncoding\n"
	psAfter = "\n$myousOk = $?; $myousCode = $LASTEXITCODE\n" +
		"if (-not $myousOk) { if ($myousCode) { exit $myousCode } else { exit 1 } }\n" +
		"exit 0\n"
)

// encodedCommand is PowerShell's -EncodedCommand argument for cmd:
// base64 of the script in UTF-16LE.
func encodedCommand(cmd string) string {
	units := utf16.Encode([]rune(psBefore + cmd + psAfter))
	raw := make([]byte, 2*len(units))
	for i, u := range units {
		binary.LittleEndian.PutUint16(raw[2*i:], u)
	}
	return base64.StdEncoding.EncodeToString(raw)
}

// shellCommand runs cmd with PowerShell in a process group of its own and
// no console window.
func shellCommand(cmd string) *exec.Cmd {
	c := exec.Command("powershell.exe", "-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
		"-EncodedCommand", encodedCommand(cmd))
	c.SysProcAttr = &syscall.SysProcAttr{CreationFlags: windows.CREATE_NEW_PROCESS_GROUP | windows.CREATE_NO_WINDOW}
	return c
}

// killTree ends a command started by shellCommand and everything it
// started (taskkill /T), then the process itself in case taskkill wasn't
// there.
func killTree(c *exec.Cmd) {
	k := exec.Command("taskkill", "/T", "/F", "/PID", strconv.Itoa(c.Process.Pid))
	k.SysProcAttr = &syscall.SysProcAttr{CreationFlags: windows.CREATE_NO_WINDOW}
	k.Run()
	c.Process.Kill()
}

// hookCommand runs a review command as Python's shell=True does on
// Windows: `%COMSPEC% /c "CMD"`, without a window.
func hookCommand(ctx context.Context, cmd string) *exec.Cmd {
	comspec := os.Getenv("COMSPEC")
	if comspec == "" {
		comspec = "cmd.exe"
	}
	c := exec.CommandContext(ctx, comspec)
	c.SysProcAttr = &syscall.SysProcAttr{CmdLine: fmt.Sprintf(`%s /c "%s"`, comspec, cmd), HideWindow: true}
	return c
}

func exitCode(ps *os.ProcessState) int { return ps.ExitCode() }

// removeEarly does nothing on Windows: an open file can't be removed, so
// dropOutput removes it after closing.
func removeEarly(f *os.File) {}

// machine is Python's platform.machine() on Windows.
func machine() string {
	if m := os.Getenv("PROCESSOR_ARCHITEW6432"); m != "" {
		return m
	}
	return os.Getenv("PROCESSOR_ARCHITECTURE")
}

// hideWindow keeps a helper program from opening a console window: the
// worker may run without one (started by the desktop app).
func hideWindow(c *exec.Cmd) {
	c.SysProcAttr = &syscall.SysProcAttr{CreationFlags: windows.CREATE_NO_WINDOW}
}

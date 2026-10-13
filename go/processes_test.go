package myous

// The shell, stopping and locks on this system: a port of
// python/tests/processes_test.py (its pid checks are the listener's, which
// the Go client doesn't have).

import (
	"runtime"
	"strings"
	"testing"
	"time"
)

var win = runtime.GOOS == "windows"

func runIn(t *testing.T, cmd string, timeout int) commandResult {
	t.Helper()
	r, err := runCommand(cmd, t.TempDir(), timeout, "")
	if err != nil {
		t.Fatal(err)
	}
	return r
}

func TestShellOutputAndExit(t *testing.T) {
	cmd := "echo héllo; exit 3"
	if win {
		cmd = "Write-Output 'héllo'; exit 3"
	}
	r := runIn(t, cmd, 30)
	if r.Exit != 3 || strings.TrimSpace(r.Stdout) != "héllo" {
		t.Fatalf("%+v", r)
	}
}

func TestShellFailingProgramSetsExit(t *testing.T) {
	cmd := `/bin/sh -c "exit 7"`
	if win {
		cmd = `& cmd.exe /c "exit 7"`
	}
	if r := runIn(t, cmd, 30); r.Exit != 7 {
		t.Fatalf("%+v", r)
	}
}

func TestShellSuccessIsZero(t *testing.T) {
	cmd := "true"
	if win {
		cmd = "Get-Location | Out-Null"
	}
	if r := runIn(t, cmd, 30); r.Exit != 0 {
		t.Fatalf("%+v", r)
	}
}

func TestShellTimeoutEndsTheChildren(t *testing.T) {
	cmd := "sleep 30"
	if win {
		cmd = "Start-Sleep -Seconds 30"
	}
	started := time.Now()
	r := runIn(t, cmd, 2)
	if time.Since(started) > 20*time.Second || r.Exit != -1 || !strings.Contains(r.Stderr, "killed after 2 seconds") {
		t.Fatalf("%+v after %v", r, time.Since(started))
	}
}

func TestShellName(t *testing.T) {
	want := "/bin/sh"
	if win {
		want = "PowerShell"
	}
	if got := shellName(isWindows); got != want {
		t.Fatal(got)
	}
}

func TestShellKilledBySignal(t *testing.T) {
	if win {
		t.Skip("POSIX signals")
	}
	// As Python reports it: minus the signal.
	if r := runIn(t, "kill -9 $$", 30); r.Exit != -9 {
		t.Fatalf("%+v", r)
	}
}

func TestShellOutputIsCut(t *testing.T) {
	out, cut := cutOutput([]byte(strings.Repeat("é", 10)), 5) // cut in the middle of a character
	if !cut || out != "éé�\n[myous: output cut at 5 bytes]" {
		t.Fatalf("%q", out)
	}
}

func TestLockSecondHolderWaits(t *testing.T) {
	st, err := NewFileStorage(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	unlock, got, err := st.Lock("state", true)
	if err != nil || !got {
		t.Fatal(got, err)
	}
	if _, got, err := st.Lock("state", false); err != nil || got {
		t.Fatalf("a second holder got the lock: %v %v", got, err)
	}
	unlock()
	unlock, got, err = st.Lock("state", false)
	if err != nil || !got {
		t.Fatal(got, err)
	}
	unlock()
}

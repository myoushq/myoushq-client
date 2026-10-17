//go:build windows

package myous

import (
	"errors"
	"strings"
	"testing"
)

func TestEncodedCommandIsPythons(t *testing.T) {
	// base64 of the script in UTF-16LE, as processes.py builds it for "echo héllo".
	want := "JABQAHIAbwBnAHIAZQBzAHMAUAByAGUAZgBlAHIAZQBuAGMAZQAgAD0AIAAnAFMAaQBsAGUAbgB0AGwAeQBDAG8AbgB0AGkAbgB1AGUAJwAKAFsAQwBvAG4AcwBvAGwAZQBdADoAOgBPAHUAdABwAHUAdABFAG4AYwBvAGQAaQBuAGcAIAA9ACAAWwBUAGUAeAB0AC4AVQBUAEYAOABFAG4AYwBvAGQAaQBuAGcAXQA6ADoAbgBlAHcAKAAkAGYAYQBsAHMAZQApAAoAJABPAHUAdABwAHUAdABFAG4AYwBvAGQAaQBuAGcAIAA9ACAAWwBDAG8AbgBzAG8AbABlAF0AOgA6AE8AdQB0AHAAdQB0AEUAbgBjAG8AZABpAG4AZwAKAGUAYwBoAG8AIABoAOkAbABsAG8ACgAkAG0AeQBvAHUAcwBPAGsAIAA9ACAAJAA/ADsAIAAkAG0AeQBvAHUAcwBDAG8AZABlACAAPQAgACQATABBAFMAVABFAFgASQBUAEMATwBEAEUACgBpAGYAIAAoAC0AbgBvAHQAIAAkAG0AeQBvAHUAcwBPAGsAKQAgAHsAIABpAGYAIAAoACQAbQB5AG8AdQBzAEMAbwBkAGUAKQAgAHsAIABlAHgAaQB0ACAAJABtAHkAbwB1AHMAQwBvAGQAZQAgAH0AIABlAGwAcwBlACAAewAgAGUAeABpAHQAIAAxACAAfQAgAH0ACgBlAHgAaQB0ACAAMAAKAA=="
	if got := encodedCommand("echo héllo"); got != want {
		t.Fatalf("got %s", got)
	}
}

func TestWindowsCommandNotFoundHint(t *testing.T) {
	// The real thing: PowerShell's error for a Linux command, and the hint after it.
	f := newWorkerFixture(t)
	r := f.exec("definitely-not-a-command-xyz")
	stderr := r["stderr"].(string)
	if r["exit"] == 0.0 || !strings.Contains(stderr, notRecognized) || !strings.HasSuffix(stderr, "\n"+windowsHint) {
		t.Fatalf("%v", r)
	}
}

// makeFIFO: no named pipes in a Windows folder.
func makeFIFO(path string) error { return errors.New("no FIFOs on Windows") }

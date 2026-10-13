//go:build !windows

package myous

import (
	"fmt"
	"os"
	"strings"
	"syscall"
	"testing"
	"time"
)

// TestWorkerHelperDaemon is not a test: the escaped-grandchild test runs
// this binary as a daemon that leaves the command's process group and
// keeps its output open.
func TestWorkerHelperDaemon(t *testing.T) {
	if os.Getenv("MYOUS_TEST_DAEMON") != "1" {
		t.Skip("helper process")
	}
	syscall.Setsid()
	fmt.Println("started")
	os.Stdout.Sync()
	time.Sleep(20 * time.Second)
	os.Exit(0)
}

func TestWorkerTimeoutDoesNotWaitForAnEscapedGrandchild(t *testing.T) {
	// A daemon that leaves the process group and keeps our output open
	// must not hang the worker: the shell is reaped, what was written stays.
	f := newWorkerFixture(t)
	cmd := fmt.Sprintf("MYOUS_TEST_DAEMON=1 %s -test.run='^TestWorkerHelperDaemon$' & wait", shlexQuote(os.Args[0]))
	started := time.Now()
	r := f.exec(cmd, "timeout", 1)
	if r["exit"] != -1.0 || !strings.Contains(r["stderr"].(string), "killed after 1 seconds") ||
		!strings.Contains(r["stdout"].(string), "started") || time.Since(started) > 12*time.Second {
		t.Fatalf("%v after %v", r, time.Since(started))
	}
}

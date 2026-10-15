//go:build !windows

package myous

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"os/signal"
	"path/filepath"
	"strconv"
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
	if r["exit"] != -1.0 || !strings.Contains(r["stderr"].(string), "[myous: killed after 1 second]") ||
		!strings.Contains(r["stdout"].(string), "started") || time.Since(started) > 12*time.Second {
		t.Fatalf("%v after %v", r, time.Since(started))
	}
}

// sleepCommand starts a sleep that writes its pid (its process group's: the
// shell execs it) to pidFile; waitPid reads it once there.
func sleepCommand(pidFile string) string {
	return fmt.Sprintf("echo begun; echo $$ > %s; exec sleep 60", shQuote(pidFile))
}

func waitPid(t *testing.T, pidFile string) int {
	t.Helper()
	for deadline := time.Now().Add(20 * time.Second); time.Now().Before(deadline); time.Sleep(50 * time.Millisecond) {
		if raw, err := os.ReadFile(pidFile); err == nil {
			if pid, err := strconv.Atoi(strings.TrimSpace(string(raw))); err == nil {
				return pid
			}
		}
	}
	t.Fatal("the command never started")
	return 0
}

func groupGone(pgid int) bool {
	return errors.Is(syscall.Kill(-pgid, 0), syscall.ESRCH)
}

func checkStoppedResult(t *testing.T, r map[string]any) {
	t.Helper()
	if r["myous"] != "result" || r["id"] != testRID || r["exit"] != -1.0 || r["stopped"] != true ||
		!strings.Contains(r["stderr"].(string), stoppingWhy) || !strings.Contains(r["stdout"].(string), "begun") {
		t.Fatalf("result: %v", r)
	}
}

func TestWorkerStopEndsTheRunningCommand(t *testing.T) {
	// The worker's context ending (SIGTERM, SIGINT) kills the command's
	// process group and answers the request.
	f := newWorkerFixture(t)
	pidFile := filepath.Join(f.tmp, "sleep.pid")
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	go func() {
		waitPid(t, pidFile)
		cancel()
	}()
	started := time.Now()
	f.w.handle(ctx, request(map[string]any{"myous": "exec", "id": testRID, "cmd": sleepCommand(pidFile), "timeout": 120}))
	if time.Since(started) > 25*time.Second {
		t.Fatalf("took %v", time.Since(started))
	}
	checkStoppedResult(t, f.last())
	if pgid := waitPid(t, pidFile); !groupGone(pgid) {
		t.Fatalf("process group %d is still there", pgid)
	}
	if rec := f.readJSON(filepath.Join(f.home, "requests", testRID+".json")); rec["stopped"] != true {
		t.Fatalf("record: %v", rec)
	}
}

// TestWorkerHelperStopped is not a test: the SIGTERM test runs this binary
// as a worker (as `myous worker` sets it up: SIGINT and SIGTERM end its
// context) whose fake agent hands it one request and writes what it sends
// to sent.json when the worker returns.
func TestWorkerHelperStopped(t *testing.T) {
	dir := os.Getenv("MYOUS_TEST_STOPPED")
	if dir == "" {
		t.Skip("helper process")
	}
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	st, err := NewFileStorage(filepath.Join(dir, "home"))
	if err != nil {
		t.Fatal(err)
	}
	agent := newFakeAgent()
	agent.listen = func(ctx context.Context, onNew func([]Entry)) error {
		onNew([]Entry{request(map[string]any{"myous": "exec", "id": testRID,
			"cmd": sleepCommand(filepath.Join(dir, "sleep.pid")), "timeout": 120})})
		<-ctx.Done()
		return ctx.Err()
	}
	w, err := newWorker(agent, st, WorkerOptions{Work: filepath.Join(dir, "work"), Out: io.Discard})
	if err != nil {
		t.Fatal(err)
	}
	if err := w.Run(ctx); err != nil {
		t.Fatal(err)
	}
	agent.mu.Lock()
	raw, _ := json.Marshal(agent.sent)
	agent.mu.Unlock()
	os.WriteFile(filepath.Join(dir, "sent.json"), raw, 0o600)
}

func TestWorkerSIGTERMEndsTheCommandAndExits(t *testing.T) {
	// SIGTERM, SIGINT, and the owner's app's commands/quit (how it stops
	// a worker on Windows) all end the command, answer, and exit 0.
	send := func(sig syscall.Signal) func(*exec.Cmd, string) {
		return func(c *exec.Cmd, _ string) { c.Process.Signal(sig) }
	}
	for _, how := range []struct {
		name string
		stop func(*exec.Cmd, string)
	}{
		{"SIGTERM", send(syscall.SIGTERM)},
		{"SIGINT", send(syscall.SIGINT)},
		{"quit file", func(_ *exec.Cmd, dir string) {
			os.MkdirAll(filepath.Join(dir, "home", "commands"), 0o700)
			os.WriteFile(filepath.Join(dir, "home", "commands", "quit"), nil, 0o600)
		}},
	} {
		t.Run(how.name, func(t *testing.T) {
			dir := t.TempDir()
			c := exec.Command(os.Args[0], "-test.run=^TestWorkerHelperStopped$")
			c.Env = append(os.Environ(), "MYOUS_TEST_STOPPED="+dir)
			out := &strings.Builder{}
			c.Stdout, c.Stderr = out, out
			if err := c.Start(); err != nil {
				t.Fatal(err)
			}
			done := make(chan error, 1)
			go func() { done <- c.Wait() }()
			pgid := waitPid(t, filepath.Join(dir, "sleep.pid"))
			if groupGone(pgid) {
				t.Fatal("the command isn't running")
			}
			started := time.Now()
			how.stop(c, dir)
			select {
			case err := <-done:
				if err != nil {
					t.Fatalf("worker: %v\n%s", err, out)
				}
			case <-time.After(10 * time.Second):
				c.Process.Kill()
				t.Fatalf("the worker didn't exit\n%s", out)
			}
			if d := time.Since(started); d > 5*time.Second {
				t.Fatalf("the worker took %v to exit", d)
			}
			if !groupGone(pgid) {
				syscall.Kill(-pgid, syscall.SIGKILL)
				t.Fatal("the command outlived the worker")
			}
			raw, err := os.ReadFile(filepath.Join(dir, "sent.json"))
			if err != nil {
				t.Fatalf("%v\n%s", err, out)
			}
			var sent [][2]string
			if err := json.Unmarshal(raw, &sent); err != nil || len(sent) != 1 {
				t.Fatalf("sent: %s", raw)
			}
			var r map[string]any
			json.Unmarshal([]byte(sent[0][1]), &r)
			checkStoppedResult(t, r)
			if exists(filepath.Join(dir, "home", "commands", "quit")) {
				t.Fatal("the quit file is still there")
			}
		})
	}
}

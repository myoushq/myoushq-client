package myous

// The Go worker against a local hub, driven by the Python reference
// client (which has `myous exec` and `myous cp`): the flow of
// python/tests/e2e_test.py's worker test, pairing included.

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestGoWorkerWithPythonAgent(t *testing.T) {
	e := setup(t)
	if e.python == "" {
		t.Skip("Python reference client not available")
	}
	if isWindows {
		t.Skip("POSIX shell commands")
	}
	e.init("py-boss")
	home := filepath.Join(e.dir, "go-wrk")
	work := filepath.Join(e.dir, "go-wrk-work")
	log, err := os.Create(filepath.Join(e.dir, "go-wrk.log"))
	if err != nil {
		t.Fatal(err)
	}
	defer log.Close()
	w := exec.Command(e.goCLI, "worker", "--alias", "go-wrk", "--hub", e.hubURL, "--work", work,
		"--description", "a test worker", "--note", "Browser: none")
	w.Env = append(os.Environ(), "MYOUS_HOME="+home)
	w.Stdout, w.Stderr = log, log
	if err := w.Start(); err != nil {
		t.Fatal(err)
	}
	defer func() { w.Process.Kill(); w.Wait() }()
	logText := func() string { b, _ := os.ReadFile(filepath.Join(e.dir, "go-wrk.log")); return string(b) }

	status := func() map[string]any {
		var s map[string]any
		raw, _ := os.ReadFile(filepath.Join(home, "worker.json"))
		json.Unmarshal(raw, &s)
		return s
	}
	waitFor := func(what string, ok func(map[string]any) bool) map[string]any {
		t.Helper()
		for i := 0; i < 200; i++ {
			if s := status(); s != nil && ok(s) {
				return s
			}
			time.Sleep(100 * time.Millisecond)
		}
		t.Fatalf("worker: no %s\n%s", what, logText())
		return nil
	}
	s := waitFor("invite", func(s map[string]any) bool {
		inv, _ := s["invite"].(map[string]any)
		return s["pid"] == float64(w.Process.Pid) && inv != nil
	})
	invite := s["invite"].(map[string]any)
	if s["phase"] != "running" || s["contacts"] != 0.0 || !strings.Contains(invite["message"].(string), `It describes itself: "a test worker"`) {
		t.Fatalf("status: %v", s)
	}

	// The owner pastes the code to their (Python) agent; the worker's
	// listener finishes the pairing.
	e.ok("py-boss", "accept", invite["code"].(string), "--relationship", "other", "--sharing", "my own worker", "--wait", "10")
	s = waitFor("pairing", func(s map[string]any) bool { return s["contacts"] == 1.0 && s["paired"] != nil })
	verify := s["paired"].(map[string]any)["verify"]
	time.Sleep(time.Second) // the card follows
	var pairedOK, card bool
	for _, x := range e.inbox("py-boss") {
		pairedOK = pairedOK || (x.Type == "paired" && x.Verify == verify && lastWord(x.Text) == verify)
		card = card || (x.Type == "card" && strings.Contains(x.Text, "a test worker"))
	}
	if !pairedOK || !card {
		t.Fatalf("the agent's inbox lacks the pairing (code %v) or the card", verify)
	}
	if s["invite"] != nil {
		t.Fatalf("an invite after pairing: %v", s)
	}

	if out := e.ok("py-boss", "exec", "go-wrk", "--", "echo", "hi"); out != "hi\n" {
		t.Fatalf("exec: %q", out)
	}
	if _, err := e.run("py-boss", "exec", "go-wrk", "--", "exit 7"); err == nil || !strings.Contains(err.Error(), "exit status 7") {
		t.Fatalf("exit 7: %v", err)
	}
	src := filepath.Join(e.dir, "in.txt")
	os.WriteFile(src, []byte("in via cp\n"), 0o600)
	if out := e.ok("py-boss", "cp", src, "go-wrk:in.txt"); !strings.Contains(out, "written on go-wrk") {
		t.Fatalf("cp in: %s", out)
	}
	if b, _ := os.ReadFile(filepath.Join(work, "in.txt")); string(b) != "in via cp\n" {
		t.Fatalf("written: %q", b)
	}
	if out := e.ok("py-boss", "exec", "go-wrk", "--", "cat in.txt"); out != "in via cp\n" {
		t.Fatalf("cat: %q", out)
	}
	dst := filepath.Join(e.dir, "out.txt")
	e.ok("py-boss", "cp", "go-wrk:in.txt", dst)
	if b, _ := os.ReadFile(dst); string(b) != "in via cp\n" {
		t.Fatalf("cp out: %q", b)
	}
	if _, err := e.run("py-boss", "cp", "go-wrk:../escape", dst); err == nil || !strings.Contains(err.Error(), "leaves the worker") {
		t.Fatalf("escape: %v", err)
	}

	// Only the help request gets an answer; the replies never reach the inbox as items.
	if got := e.inbox("py-boss"); len(got) != 0 {
		t.Fatalf("inbox before help: %+v", got)
	}
	e.ok("py-boss", "send", "go-wrk", "help")
	time.Sleep(3 * time.Second)
	got := e.inbox("py-boss")
	if len(got) != 1 || !strings.Contains(got[0].Text, "go-wrk is a myoushq worker") || !strings.Contains(got[0].Text, "Its owner describes it: a test worker") ||
		!strings.Contains(got[0].Text, "Browser: none") {
		t.Fatalf("help: %+v", got)
	}
	if s := status(); s["requests"].(float64) < 5 {
		t.Fatalf("status: %v", s)
	}
	// The owner's records and log.
	records, _ := filepath.Glob(filepath.Join(home, "requests", "*.json"))
	if len(records) < 5 {
		t.Fatalf("%d request records", len(records))
	}
	if l := logText(); !strings.Contains(l, "registering with ") || !strings.Contains(l, "paired with py-boss: verification code") {
		t.Fatalf("worker output:\n%s", l)
	}
	var settings map[string]any
	raw, _ := os.ReadFile(filepath.Join(home, "settings.json"))
	json.Unmarshal(raw, &settings)
	if settings["about"] != WorkerAbout || settings["card"] != "a test worker" {
		t.Fatalf("settings: %v", settings)
	}

	// The built-in review at the "changes" level, answered as the desktop
	// app answers (worker/desktop/src-tauri/src/files.rs): a read runs, a
	// change asks; the request's record says "pending" meanwhile.
	os.WriteFile(filepath.Join(home, "review.json"), []byte(`{"level":"changes"}`), 0o600)
	if out := e.ok("py-boss", "exec", "go-wrk", "--", "pwd"); !strings.Contains(out, "go-wrk-work") {
		t.Fatalf("pwd: %q", out)
	}
	answered := make(chan string, 1)
	go func() {
		for i := 0; i < 200; i++ {
			qs, _ := filepath.Glob(filepath.Join(home, "approvals", "*.json"))
			if len(qs) == 1 {
				id := strings.TrimSuffix(filepath.Base(qs[0]), ".json")
				var rec map[string]any
				raw, _ := os.ReadFile(filepath.Join(home, "requests", id+".json"))
				json.Unmarshal(raw, &rec)
				os.WriteFile(filepath.Join(home, "approvals", id+".answer.tmp"), []byte("allow"), 0o600)
				os.Rename(filepath.Join(home, "approvals", id+".answer.tmp"), filepath.Join(home, "approvals", id+".answer"))
				answered <- fmt.Sprint(rec["decision"])
				return
			}
			time.Sleep(100 * time.Millisecond)
		}
		answered <- "never asked"
	}()
	e.ok("py-boss", "exec", "go-wrk", "--", "echo made > made.txt")
	if d := <-answered; d != "pending" {
		t.Fatalf("while asking, the record said %q", d)
	}
	if !exists(filepath.Join(work, "made.txt")) {
		t.Fatal("the allowed change didn't run")
	}
	os.Remove(filepath.Join(home, "review.json"))

	// The owner unpairs from the app: contacts gone, a new code offered.
	os.MkdirAll(filepath.Join(home, "commands"), 0o700)
	os.WriteFile(filepath.Join(home, "commands", "unpair"), nil, 0o600)
	waitFor("new invite after unpair", func(s map[string]any) bool { return s["contacts"] == 0.0 && s["invite"] != nil })
}

func TestReplyDuringListenUsesTheLiveConnection(t *testing.T) {
	// A reply sent from onNew (as the worker sends its results) goes out on
	// the connection Listen holds: no connect, TLS and auth per reply.
	e := setup(t)
	e.init("go-lis", "go-snd")
	e.pair("go-lis", "go-snd", "code")
	st, err := NewFileStorage(filepath.Join(e.dir, "go-lis"))
	if err != nil {
		t.Fatal(err)
	}
	agent, err := New(st, "")
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	opened := make(chan int64, 1)
	go agent.Listen(ctx, func(entries []Entry) {
		for _, x := range entries {
			if x.Type == "message" && x.Text == "ping" {
				before := connects.Load()
				if _, err := agent.Send(ctx, x.Peer, "pong"); err != nil {
					t.Error(err)
				}
				opened <- connects.Load() - before
			}
		}
	}, nil, 20*time.Second)
	time.Sleep(2 * time.Second)
	e.ok("go-snd", "send", "go-lis", "ping")
	select {
	case n := <-opened:
		if n != 0 {
			t.Fatalf("the reply opened %d connection(s)", n)
		}
	case <-time.After(30 * time.Second):
		t.Fatal("no ping arrived")
	}
	got := e.inbox("go-snd")
	if len(got) != 1 || got[0].Text != "pong" {
		t.Fatalf("the reply didn't arrive: %+v", got)
	}
	// With Listen gone, a send opens its own connection again.
	cancel()
	time.Sleep(500 * time.Millisecond)
	before := connects.Load()
	if _, err := agent.Send(context.Background(), "go-snd", "after"); err != nil {
		t.Fatal(err)
	}
	if connects.Load()-before != 1 {
		t.Fatalf("%d connections after Listen", connects.Load()-before)
	}
}

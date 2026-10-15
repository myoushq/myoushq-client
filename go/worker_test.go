package myous

// The worker's request handling, with a fake agent: no hub, no relay. A
// port of python/tests/worker_test.py, case by case, plus checks that the
// Go worker writes what the Python one writes.

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/nbd-wtf/go-nostr"
)

const (
	testPeer = "npub1peer"
	testRID  = "abababababababababababababababab"
)

type sentFile struct {
	to, path string
	tags     nostr.Tags
}

type fakeAgent struct {
	mu       sync.Mutex
	alias    string
	card     string
	sent     [][2]string
	files    []sentFile
	blobs    map[string][]byte
	contacts map[string]Contact
	invites  int
	failSend map[string]error // Send to these fails
	fetches  int              // FetchBytes calls
	listen   func(ctx context.Context, onNew func([]Entry)) error
}

func newFakeAgent() *fakeAgent {
	return &fakeAgent{alias: "box", blobs: map[string][]byte{},
		contacts: map[string]Contact{"k": {Alias: "muse", Npub: testPeer, Status: Approved}}}
}

func (f *fakeAgent) Alias() string         { return f.alias }
func (f *fakeAgent) Card() string          { return f.card }
func (f *fakeAgent) Npub() (string, error) { return "npub1box", nil }
func (f *fakeAgent) Contacts() (map[string]Contact, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	out := map[string]Contact{}
	for k, v := range f.contacts {
		out[k] = v
	}
	return out, nil
}
func (f *fakeAgent) setContacts(c map[string]Contact) {
	f.mu.Lock()
	f.contacts = c
	f.mu.Unlock()
}
func (f *fakeAgent) Invite(ctx context.Context, cc ...ContactContext) (*Invite, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.invites++
	return &Invite{Pending: Pending{ExpiresAt: time.Now().Unix() + 900}, Code: fmt.Sprintf("1000-AAAAA%d", f.invites),
		Link: "https://hub.test/p/1000#AAAAA"}, nil
}
func (f *fakeAgent) Send(ctx context.Context, name, text string) (Entry, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	if err := f.failSend[name]; err != nil {
		return Entry{}, err
	}
	f.sent = append(f.sent, [2]string{name, text})
	return Entry{}, nil
}
func (f *fakeAgent) SyncCards(ctx context.Context) ([]Contact, error) { return nil, nil }
func (f *fakeAgent) SendFile(ctx context.Context, name, path string, tags nostr.Tags) (Entry, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.files = append(f.files, sentFile{name, path, tags})
	return Entry{}, nil
}
func (f *fakeAgent) FetchBytes(ctx context.Context, e Entry) ([]byte, error) {
	f.mu.Lock()
	f.fetches++
	f.mu.Unlock()
	b, ok := f.blobs[e.X]
	if !ok {
		return nil, errors.New("no such blob")
	}
	return b, nil
}
func (f *fakeAgent) Listen(ctx context.Context, onNew func([]Entry), onTick func(), tick time.Duration) error {
	if f.listen != nil {
		return f.listen(ctx, onNew)
	}
	return errors.New("no relay in tests")
}

func (f *fakeAgent) sentCount() int {
	f.mu.Lock()
	defer f.mu.Unlock()
	return len(f.sent)
}

type workerFixture struct {
	t     *testing.T
	w     *Worker
	agent *fakeAgent
	st    *FileStorage
	home  string
	work  string // as the worker resolved it
	tmp   string
	ctx   context.Context
	mtime time.Time // browser.json's last (newBrowser)
}

func newWorkerFixture(t *testing.T) *workerFixture {
	t.Helper()
	t.Setenv("MYOUS_PAUSE_FILE", "")
	t.Setenv("MYOUS_APPROVAL_WAIT", "")
	tmp, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	st, err := NewFileStorage(filepath.Join(tmp, "home"))
	if err != nil {
		t.Fatal(err)
	}
	agent := newFakeAgent()
	w, err := newWorker(agent, st, WorkerOptions{Work: filepath.Join(tmp, "work"),
		Notes: []string{"Browser: http://localhost:9222"}, Out: io.Discard})
	if err != nil {
		t.Fatal(err)
	}
	return &workerFixture{t: t, w: w, agent: agent, st: st, home: st.Home, work: w.work, tmp: tmp, ctx: context.Background()}
}

func textEntry(text string) Entry {
	return Entry{Type: "message", Direction: "in", Peer: testPeer, Alias: "muse", Text: text, Seq: 1}
}

func request(fields map[string]any) Entry {
	b, _ := json.Marshal(fields)
	return textEntry(string(b))
}

// replies are what the worker sent, JSON ones decoded.
func (f *workerFixture) replies() []any {
	f.agent.mu.Lock()
	defer f.agent.mu.Unlock()
	var out []any
	for _, s := range f.agent.sent {
		var v map[string]any
		if strings.HasPrefix(s[1], "{") && json.Unmarshal([]byte(s[1]), &v) == nil {
			out = append(out, v)
		} else {
			out = append(out, s[1])
		}
	}
	return out
}

func (f *workerFixture) last() map[string]any {
	f.t.Helper()
	r := f.replies()
	if len(r) == 0 {
		f.t.Fatal("no reply")
	}
	m, ok := r[len(r)-1].(map[string]any)
	if !ok {
		f.t.Fatalf("last reply isn't JSON: %v", r[len(r)-1])
	}
	return m
}

func (f *workerFixture) lastText() string {
	f.agent.mu.Lock()
	defer f.agent.mu.Unlock()
	return f.agent.sent[len(f.agent.sent)-1][1]
}

func (f *workerFixture) exec(cmd string, extra ...any) map[string]any {
	f.t.Helper()
	req := map[string]any{"myous": "exec", "id": testRID, "cmd": cmd}
	for i := 0; i+1 < len(extra); i += 2 {
		req[extra[i].(string)] = extra[i+1]
	}
	f.w.handle(f.ctx, request(req))
	return f.last()
}

func (f *workerFixture) readJSON(path string) map[string]any {
	f.t.Helper()
	raw, err := os.ReadFile(path)
	if err != nil {
		f.t.Fatal(err)
	}
	var v map[string]any
	if err := json.Unmarshal(raw, &v); err != nil {
		f.t.Fatalf("%s: %v", path, err)
	}
	return v
}

func (f *workerFixture) status() map[string]any {
	return f.readJSON(filepath.Join(f.home, "worker.json"))
}

func (f *workerFixture) logLines() []map[string]any {
	f.t.Helper()
	raw, err := os.ReadFile(filepath.Join(f.home, "worker.log"))
	if err != nil {
		f.t.Fatal(err)
	}
	var out []map[string]any
	for _, line := range strings.Split(strings.TrimSpace(string(raw)), "\n") {
		var v map[string]any
		if err := json.Unmarshal([]byte(line), &v); err != nil {
			f.t.Fatalf("worker.log line %q: %v", line, err)
		}
		out = append(out, v)
	}
	return out
}

func (f *workerFixture) writeStatus() {
	f.w.mu.Lock()
	defer f.w.mu.Unlock()
	if err := f.w.writeStatus(); err != nil {
		f.t.Fatal(err)
	}
}

func (f *workerFixture) ensureInvite() {
	f.w.mu.Lock()
	defer f.w.mu.Unlock()
	if err := f.w.ensureInvite(f.ctx); err != nil {
		f.t.Fatal(err)
	}
}

func needPOSIX(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("POSIX shell commands")
	}
}

func TestWorkerExecResult(t *testing.T) {
	needPOSIX(t)
	f := newWorkerFixture(t)
	r := f.exec("echo out; echo err >&2; exit 3")
	if r["myous"] != "result" || r["id"] != testRID || r["exit"] != 3.0 || r["stdout"] != "out\n" || r["stderr"] != "err\n" || r["truncated"] != false {
		t.Fatalf("result: %v", r)
	}
	// Exactly what the Python worker sends (json.dumps of the same dict).
	want := `{"exit": 3, "stdout": "out\n", "stderr": "err\n", "truncated": false, "myous": "result", "id": "` + testRID + `"}`
	if got := f.lastText(); got != want {
		t.Fatalf("reply text\n got %s\nwant %s", got, want)
	}
	if f.agent.sent[0][0] != testPeer {
		t.Fatalf("sent to %s", f.agent.sent[0][0])
	}
	log := f.logLines()
	if l := log[len(log)-1]; l["op"] != "exec" || l["decision"] != "allow" || l["alias"] != "muse" {
		t.Fatalf("log: %v", l)
	}
	f.writeStatus()
	s := f.status()
	if s["requests"] != 1.0 || s["last"].(map[string]any)["op"] != "exec" || s["contacts"] != 1.0 || s["invite"] != nil {
		t.Fatalf("status: %v", s)
	}
}

func TestWorkerPairingMessage(t *testing.T) {
	m := pairingMessage("1234-ABCDE", "Sam's Mac", "", false)
	if !strings.Contains(m, "1234-ABCDE") || !strings.Contains(m, "not something to run yourself") || strings.Contains(m, "describes itself") {
		t.Fatal(m)
	}
	// Exactly the Python worker's sentence.
	want := `Pair with my worker "Sam's Mac" (an environment I set up for you, not something to run yourself): accept the pairing code 1234-ABCDE with relationship other and sharing "my own worker; run commands there for me". It describes itself: "Sam's own Mac". Then send it the message help, and read the Workers section of https://myoushq.com/skill.md before using it.`
	if got := pairingMessage("1234-ABCDE", "Sam's Mac", "Sam's own Mac", false); got != want {
		t.Fatalf("got  %s\nwant %s", got, want)
	}
}

func TestWorkerHelpAndPairingCarryTheDescription(t *testing.T) {
	f := newWorkerFixture(t)
	f.agent.card = "Sam's own Mac; its browser is the 'myous browser'"
	f.w.handle(f.ctx, textEntry("help"))
	if !strings.Contains(f.agent.sent[0][1], "Its owner describes it: Sam's own Mac; its browser is the 'myous browser'") {
		t.Fatal(f.agent.sent[0][1])
	}
	f.agent.setContacts(map[string]Contact{})
	f.ensureInvite()
	f.writeStatus()
	msg := f.status()["invite"].(map[string]any)["message"].(string)
	if !strings.Contains(msg, `It describes itself: "Sam's own Mac; its browser is the 'myous browser'"`) {
		t.Fatal(msg)
	}
}

func TestWorkerExecRunsInWorkDirAndTruncates(t *testing.T) {
	needPOSIX(t)
	f := newWorkerFixture(t)
	r := f.exec(`pwd; head -c 200000 /dev/zero | tr '\0' x`)
	out := r["stdout"].(string)
	if !strings.HasPrefix(out, f.work+"\n") || r["truncated"] != true || !strings.Contains(out, "output cut") || len(out) >= stdoutLimit+200 {
		t.Fatalf("stdout starts %q (%d bytes), truncated %v", out[:min(len(out), 80)], len(out), r["truncated"])
	}
}

func TestWorkerExecTimeoutKillsTheCommand(t *testing.T) {
	needPOSIX(t)
	f := newWorkerFixture(t)
	started := time.Now()
	r := f.exec("sleep 30", "timeout", 1)
	if r["exit"] != -1.0 || !strings.Contains(r["stderr"].(string), "killed after 1 seconds") || time.Since(started) > 10*time.Second {
		t.Fatalf("%v after %v", r, time.Since(started))
	}
}

func TestWorkerOwnerStopsARunningCommand(t *testing.T) {
	needPOSIX(t)
	// The app drops commands/stop-<id>; the worker kills the command and
	// tells the agent it was stopped by the owner.
	f := newWorkerFixture(t)
	stop := filepath.Join(f.home, "commands", "stop-"+testRID)
	os.MkdirAll(filepath.Dir(stop), 0o700)
	go func() {
		time.Sleep(700 * time.Millisecond)
		os.WriteFile(stop, nil, 0o600)
	}()
	started := time.Now()
	r := f.exec("echo begun; sleep 30", "timeout", 60)
	if r["exit"] != -1.0 || r["stopped"] != true || !strings.Contains(r["stderr"].(string), "stopped by the owner") ||
		!strings.Contains(r["stdout"].(string), "begun") || time.Since(started) > 10*time.Second {
		t.Fatalf("%v after %v", r, time.Since(started))
	}
	if exists(stop) {
		t.Fatal("the stop file is still there")
	}
	if rec := f.readJSON(filepath.Join(f.home, "requests", testRID+".json")); rec["stopped"] != true {
		t.Fatalf("record: %v", rec)
	}
}

func TestWorkerARequestIDCannotNameAStopFileElsewhere(t *testing.T) {
	needPOSIX(t)
	// "stop-../../../key" would be <home>/key wherever ".." is resolved by
	// name (filepath.Join, Windows; here also through a directory called "stop-..").
	f := newWorkerFixture(t)
	os.MkdirAll(filepath.Join(f.home, "commands", "stop-.."), 0o700)
	victim := filepath.Join(f.home, "victim")
	os.WriteFile(victim, []byte("keep me"), 0o600)
	f.w.handle(f.ctx, request(map[string]any{"myous": "exec", "id": "../../../victim", "cmd": "echo ran"}))
	if r := f.last(); r["ok"] != false || r["error"] != badID || r["id"] != "../../../victim" {
		t.Fatalf("%v", r)
	}
	if b, err := os.ReadFile(victim); err != nil || string(b) != "keep me" {
		t.Fatalf("the file outside commands/: %q %v", b, err)
	}
}

func TestWorkerRequestIDsArePlainNames(t *testing.T) {
	needPOSIX(t)
	// One rule for ids, as the apps and review.py have it: 1 to 64 of
	// A-Z, a-z, 0-9, - and _. Anything else is refused before it runs.
	f := newWorkerFixture(t)
	good := []string{"a-b_C9", strings.Repeat("x", 64), "0"}
	for _, rid := range good {
		f.w.handle(f.ctx, request(map[string]any{"myous": "exec", "id": rid, "cmd": "echo ran"}))
		if r := f.last(); r["stdout"] != "ran\n" || !exists(filepath.Join(f.home, "requests", rid+".json")) {
			t.Fatalf("%q: %v", rid, r)
		}
	}
	for _, rid := range []string{strings.Repeat("x", 65), "", "caf\u00e9", "\u0661\u0662", "a.b", "a b", "a/b", `a\b`, "a\n"} {
		f.w.handle(f.ctx, request(map[string]any{"myous": "exec", "id": rid, "cmd": "touch ran"}))
		want := pyDumps(pyObj{{"myous", "ack"}, {"id", rid}, {"ok", false}, {"error", badID}}, false)
		if got := f.lastText(); got != want {
			t.Fatalf("%q: %s", rid, got)
		}
	}
	if exists(filepath.Join(f.work, "ran")) {
		t.Fatal("a request with a bad id ran")
	}
	f.w.handle(f.ctx, Entry{Type: "file", Direction: "in", Peer: testPeer, Alias: "muse", Name: "in.txt", Size: 1,
		W: []string{"put", "a.b", "in.txt"}, Seq: 2})
	if r := f.last(); r["ok"] != false || r["error"] != badID || r["id"] != "a.b" {
		t.Fatalf("put: %v", r)
	}
	entries, _ := os.ReadDir(filepath.Join(f.home, "requests"))
	var names []string
	for _, e := range entries {
		names = append(names, e.Name())
	}
	if got := strings.Join(names, " "); got != "0.json a-b_C9.json "+strings.Repeat("x", 64)+".json" {
		t.Fatalf("records: %s", got)
	}
	for id, ok := range map[string]bool{"ab": true, "A-_9": true, "": false, "é": false, "a.b": false,
		strings.Repeat("z", 64): true, strings.Repeat("z", 65): false} {
		if validID(id) != ok {
			t.Fatalf("validID(%q) != %v", id, ok)
		}
	}
}

func TestWorkerStoppingEndsAReviewThatWaits(t *testing.T) {
	needPOSIX(t)
	// A review hook waiting for the owner when the worker is stopped:
	// ended, and the agent is told.
	f := newWorkerFixture(t)
	f.w.reviewCmd = "sleep 30; :" // two commands: the shell stays the parent (as dash on Linux does for one), so the group kill is what ends the sleep
	ctx, cancel := context.WithCancel(context.Background())
	time.AfterFunc(500*time.Millisecond, cancel)
	started := time.Now()
	f.w.handle(ctx, request(map[string]any{"myous": "exec", "id": testRID, "cmd": "touch ran"}))
	if got, want := f.lastText(), `{"myous": "ack", "id": "`+testRID+`", "ok": false, "error": "`+stoppingWhy+`"}`; got != want {
		t.Fatalf("got %s", got)
	}
	if exists(filepath.Join(f.work, "ran")) || time.Since(started) > 5*time.Second {
		t.Fatalf("ran, or took %v", time.Since(started))
	}
}

// runUntilStopped runs the worker with a fake connection that hands it
// one exec request, until Run returns; the result comes on the channel.
func (f *workerFixture) runUntilStopped(cmd string) <-chan error {
	f.agent.listen = func(ctx context.Context, onNew func([]Entry)) error {
		onNew([]Entry{request(map[string]any{"myous": "exec", "id": testRID, "cmd": cmd, "timeout": 120})})
		<-ctx.Done()
		return ctx.Err()
	}
	done := make(chan error, 1)
	go func() { done <- f.w.Run(context.Background()) }()
	return done
}

func TestWorkerQuitFileStopsTheWorker(t *testing.T) {
	// The owner's app creates commands/quit (on Windows its only way to
	// stop the worker gracefully): the command running is ended, the
	// agent is told, the file is removed and Run returns, as on SIGTERM.
	f := newWorkerFixture(t)
	cmd := "echo begun; echo > started; sleep 60"
	if runtime.GOOS == "windows" {
		cmd = "Write-Output begun; New-Item started -ItemType File | Out-Null; Start-Sleep 60"
	}
	done := f.runUntilStopped(cmd)
	for deadline := time.Now().Add(60 * time.Second); !exists(filepath.Join(f.work, "started")); time.Sleep(50 * time.Millisecond) {
		if time.Now().After(deadline) {
			t.Fatal("the command never started")
		}
	}
	quit := filepath.Join(f.home, "commands", "quit")
	os.MkdirAll(filepath.Dir(quit), 0o700)
	os.WriteFile(quit, nil, 0o600)
	started := time.Now()
	select {
	case err := <-done:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(20 * time.Second):
		t.Fatal("the worker didn't stop")
	}
	if d := time.Since(started); d > 8*time.Second {
		t.Fatalf("the worker took %v to stop", d)
	}
	r := f.last()
	if r["myous"] != "result" || r["id"] != testRID || r["exit"] != -1.0 || r["stopped"] != true ||
		!strings.Contains(r["stderr"].(string), stoppingWhy) || !strings.Contains(r["stdout"].(string), "begun") {
		t.Fatalf("result: %v", r)
	}
	if exists(quit) {
		t.Fatal("the quit file is still there")
	}
}

func TestWorkerQuitFileFromBeforeItStartedIsDropped(t *testing.T) {
	// A quit file the last worker never took (it was killed first) must
	// not stop the next one; one written since it started does.
	f := newWorkerFixture(t)
	quit := filepath.Join(f.home, "commands", "quit")
	os.MkdirAll(filepath.Dir(quit), 0o700)
	os.WriteFile(quit, nil, 0o600)
	// The other commands leave it to Run's watcher.
	f.w.mu.Lock()
	f.w.handleCommands(context.Background())
	f.w.mu.Unlock()
	if !exists(quit) {
		t.Fatal("handleCommands took the quit file")
	}
	old := time.Now().Add(-time.Hour)
	os.Chtimes(quit, old, old)
	f.agent.listen = func(ctx context.Context, onNew func([]Entry)) error {
		<-ctx.Done()
		return ctx.Err()
	}
	done := make(chan error, 1)
	go func() { done <- f.w.Run(context.Background()) }()
	select {
	case err := <-done:
		t.Fatalf("stopped by a stale quit file: %v", err)
	case <-time.After(4 * stopEvery):
	}
	if exists(quit) {
		t.Fatal("the stale quit file is still there")
	}
	os.WriteFile(quit, nil, 0o600)
	select {
	case err := <-done:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("the worker didn't stop")
	}
}

func TestWorkerMalformedRequestsAreRefusedNotFatal(t *testing.T) {
	needPOSIX(t)
	f := newWorkerFixture(t)
	f.w.handle(f.ctx, textEntry(`{"myous": "exec", "id": "`+testRID+`", "cmd": "echo ok", "timeout": Infinity}`))
	if r := f.last(); r["stdout"] != "ok\n" { // a bad timeout means the default
		t.Fatalf("Infinity timeout: %v", r)
	}
	f.w.handle(f.ctx, textEntry(`{"myous": "exec", "id": "`+testRID+`", "cmd": "echo big", "timeout": 1e400}`))
	if r := f.last(); r["stdout"] != "big\n" {
		t.Fatalf("1e400 timeout: %v", r)
	}
	f.w.handle(f.ctx, request(map[string]any{"myous": "get", "id": testRID, "path": "a\x00b"}))
	if r := f.last(); r["ok"] != false {
		t.Fatalf("NUL path: %v", r)
	}
	f.w.handle(f.ctx, request(map[string]any{"myous": "exec", "id": testRID, "cmd": []string{"not", "a", "string"}}))
	if r := f.last(); r["ok"] != false || r["error"] != "exec needs a cmd string" {
		t.Fatalf("list cmd: %v", r)
	}
	// Something unexpected inside an operation: a refusal, and the worker lives on.
	f.w.execFn = nil
	f.w.handle(f.ctx, request(map[string]any{"myous": "exec", "id": testRID, "cmd": "echo x"}))
	if r := f.last(); !strings.Contains(r["error"].(string), "bad request") {
		t.Fatalf("panic: %v", r)
	}
	// A work directory a command removed comes back.
	os.RemoveAll(f.work)
	f.w.handle(f.ctx, request(map[string]any{"myous": "get", "id": testRID, "path": "x"}))
	if info, err := os.Stat(f.work); err != nil || !info.IsDir() {
		t.Fatal("the work directory didn't come back")
	}
}

func TestWorkerReviewHookMustLiveOutsideTheWorkDir(t *testing.T) {
	needPOSIX(t)
	f := newWorkerFixture(t)
	inside := filepath.Join(f.work, "review.sh")
	os.WriteFile(inside, []byte("exit 0\n"), 0o700)
	if _, err := newWorker(f.agent, f.st, WorkerOptions{Work: f.work, Review: "/bin/sh " + inside, Out: io.Discard}); err == nil ||
		!strings.Contains(err.Error(), "is inside the work directory") {
		t.Fatalf("a hook inside the work directory: %v", err)
	}
	// A relative hook outside it is made absolute, and runs from the home directory, not the work directory.
	outside := filepath.Join(f.tmp, "hook.sh")
	os.WriteFile(outside, []byte("pwd -P >&2; exit 1\n"), 0o700)
	t.Chdir(f.tmp)
	w, err := newWorker(f.agent, f.st, WorkerOptions{Work: f.work, Review: "/bin/sh hook.sh", Out: io.Discard})
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(w.reviewCmd, outside) {
		t.Fatalf("review command %q", w.reviewCmd)
	}
	w.handle(f.ctx, request(map[string]any{"myous": "exec", "id": testRID, "cmd": "echo x"}))
	if r := f.last(); r["error"] != f.home {
		t.Fatalf("the hook ran in %v, not %s", r["error"], f.home)
	}
}

func TestWorkerRequestsRunOneAtATime(t *testing.T) {
	needPOSIX(t)
	f := newWorkerFixture(t)
	marks := filepath.Join(f.work, "marks")
	first := request(map[string]any{"myous": "exec", "id": strings.Repeat("a", 32),
		"cmd": fmt.Sprintf("echo start1 >> %s; sleep 1; echo end1 >> %s", marks, marks)})
	second := request(map[string]any{"myous": "exec", "id": strings.Repeat("b", 32),
		"cmd": fmt.Sprintf("echo start2 >> %s; echo end2 >> %s", marks, marks)})
	var wg sync.WaitGroup
	wg.Add(2)
	go func() { defer wg.Done(); f.w.onNew(f.ctx, []Entry{first}) }()
	for !exists(marks) { // the first has begun
		time.Sleep(10 * time.Millisecond)
	}
	go func() { defer wg.Done(); f.w.onNew(f.ctx, []Entry{second}) }()
	wg.Wait()
	raw, _ := os.ReadFile(marks)
	if got := strings.Fields(string(raw)); strings.Join(got, " ") != "start1 end1 start2 end2" {
		t.Fatalf("marks: %v", got)
	}
}

func TestWorkerReviewHookRefuses(t *testing.T) {
	needPOSIX(t)
	f := newWorkerFixture(t)
	hook := filepath.Join(f.tmp, "review.sh")
	seen := filepath.Join(f.tmp, "seen.json")
	os.WriteFile(hook, []byte(fmt.Sprintf("tee %s | grep -q '\"cmd\": \"rm' && { echo no deleting >&2; echo second line >&2; exit 1; }\nexit 0\n", seen)), 0o700)
	f.w.reviewCmd = "/bin/sh " + hook
	f.exec("rm -rf x")
	if got, want := f.lastText(), `{"myous": "ack", "id": "`+testRID+`", "ok": false, "error": "no deleting"}`; got != want {
		t.Fatalf("got %s\nwant %s", got, want)
	}
	if r := f.exec("echo fine é"); r["stdout"] != "fine é\n" {
		t.Fatalf("%v", r)
	}
	// The hook gets the request exactly as the Python worker writes it.
	raw, _ := os.ReadFile(seen)
	if want := `{"op": "exec", "id": "` + testRID + `", "sender": "npub1peer", "alias": "muse", "cmd": "echo fine \u00e9"}`; string(raw) != want {
		t.Fatalf("hook input\n got %s\nwant %s", raw, want)
	}
}

func TestWorkerPauseFile(t *testing.T) {
	needPOSIX(t)
	f := newWorkerFixture(t)
	pause := filepath.Join(f.home, "worker.paused")
	os.WriteFile(pause, nil, 0o600)
	// Without --review the worker applies review.py's rules, so the reason
	// is review.py's (the Python worker without a hook says "worker is paused").
	if r := f.exec("echo x"); r["error"] != pausedByOwner {
		t.Fatalf("%v", r)
	}
	os.Remove(pause)
	if r := f.exec("echo x"); r["stdout"] != "x\n" {
		t.Fatalf("%v", r)
	}
}

func TestWorkerPutAndGet(t *testing.T) {
	f := newWorkerFixture(t)
	enc, err := EncryptFile([]byte("contents"))
	if err != nil {
		t.Fatal(err)
	}
	f.agent.blobs[enc.X] = []byte("contents")
	f.w.handle(f.ctx, Entry{Type: "file", Direction: "in", Peer: testPeer, Alias: "muse", Name: "in.txt", Size: 24,
		X: enc.X, Ox: enc.Ox, W: []string{"put", testRID, "sub/in.txt"}, Seq: 2})
	ack := f.last()
	if ack["ok"] != true || ack["size"] != 8.0 || ack["sha256"] != sha256Hex([]byte("contents")) {
		t.Fatalf("ack: %v", ack)
	}
	want := filepath.Join(f.work, "sub", "in.txt")
	if ack["path"] != want {
		t.Fatalf("path %v, want %s", ack["path"], want)
	}
	if b, _ := os.ReadFile(want); string(b) != "contents" {
		t.Fatalf("written: %q", b)
	}

	f.w.handle(f.ctx, request(map[string]any{"myous": "get", "id": testRID, "path": "sub/in.txt"}))
	got := f.agent.files[len(f.agent.files)-1]
	if got.to != testPeer || got.path != want || len(got.tags) != 1 || strings.Join(got.tags[0], ",") != "w,file,"+testRID {
		t.Fatalf("sent file: %+v", got)
	}
	f.w.handle(f.ctx, request(map[string]any{"myous": "get", "id": testRID, "path": "missing.txt"}))
	if r := f.last(); r["error"] != "no such file on the worker" {
		t.Fatalf("%v", r)
	}
}

func TestWorkerPathsCannotEscape(t *testing.T) {
	f := newWorkerFixture(t)
	for _, bad := range []string{"../x", "/etc/passwd", "a/../../x", "", "   "} {
		f.w.handle(f.ctx, request(map[string]any{"myous": "get", "id": testRID, "path": bad}))
		if r := f.last(); r["ok"] != false {
			t.Fatalf("%q: %v", bad, r)
		}
	}
	if runtime.GOOS != "windows" {
		// A link is followed before "..": work/link/../x is /x when link points at /etc, as in Python.
		os.Symlink("/etc", filepath.Join(f.work, "link"))
		for _, bad := range []string{"link/passwd", "link/../x"} {
			if _, why := f.w.resolve(bad); why != "the path leaves the worker's work directory" {
				t.Fatalf("%q: %q", bad, why)
			}
		}
		if target, why := f.w.resolve("a/./b//c/"); why != "" || target != filepath.Join(f.work, "a", "b", "c") {
			t.Fatalf("%q %q", target, why)
		}
	}
	f.w.allowAbsolute = true
	abs := "/etc/passwd"
	if runtime.GOOS == "windows" {
		abs = `C:\Windows\win.ini`
	}
	if target, why := f.w.resolve(abs); target != abs || why != "" {
		t.Fatalf("absolute: %q %q", target, why)
	}
}

func TestWorkerHelpOnceAMinute(t *testing.T) {
	f := newWorkerFixture(t)
	f.w.handle(f.ctx, textEntry("help"))
	f.w.handle(f.ctx, textEntry("what can you do?"))
	if n := f.agent.sentCount(); n != 1 {
		t.Fatalf("%d replies", n)
	}
	text := f.agent.sent[0][1]
	for _, want := range []string{"myoushq worker", "Browser: http://localhost:9222", "myous exec", "Shell: " + shellName(isWindows) + " on "} {
		if !strings.Contains(text, want) {
			t.Fatalf("help lacks %q:\n%s", want, text)
		}
	}
	f.w.helpSent[testPeer] = time.Now().Add(-helpInterval - time.Second)
	f.w.handle(f.ctx, textEntry("help"))
	if n := f.agent.sentCount(); n != 2 {
		t.Fatalf("%d replies", n)
	}
	// A reply addressed to us by mistake is ignored, not answered with help.
	f.w.handle(f.ctx, request(map[string]any{"myous": "ack", "id": testRID, "ok": true}))
	if n := f.agent.sentCount(); n != 2 {
		t.Fatalf("%d replies", n)
	}
}

func TestWorkerInviteUntilPaired(t *testing.T) {
	f := newWorkerFixture(t)
	f.agent.setContacts(map[string]Contact{})
	f.ensureInvite()
	if f.agent.invites != 1 {
		t.Fatal(f.agent.invites)
	}
	f.ensureInvite()
	if f.agent.invites != 1 { // still valid
		t.Fatal(f.agent.invites)
	}
	f.w.invite.ExpiresAt = time.Now().Unix()
	f.ensureInvite()
	if f.agent.invites != 2 {
		t.Fatal(f.agent.invites)
	}
	if code := f.status()["invite"].(map[string]any)["code"]; code != "1000-AAAAA2" {
		t.Fatal(code)
	}
	f.agent.setContacts(map[string]Contact{"k": {Alias: "muse", Npub: testPeer, Status: Approved}})
	f.ensureInvite()
	if f.w.invite != nil {
		t.Fatal("an invite while paired")
	}
}

func TestWorkerPairingResultShowsTheVerificationCode(t *testing.T) {
	f := newWorkerFixture(t)
	f.w.onNew(f.ctx, []Entry{{Type: "paired", Alias: "muse", Peer: testPeer, Verify: "358806", Text: "paired"}})
	p := f.status()["paired"].(map[string]any)
	if p["alias"] != "muse" || p["verify"] != "358806" {
		t.Fatalf("%v", p)
	}
	log := f.logLines()
	if l := log[len(log)-1]; l["op"] != "paired" || l["verify"] != "358806" {
		t.Fatalf("%v", l)
	}
}

func TestWorkerStatusHasAPhase(t *testing.T) {
	f := newWorkerFixture(t)
	f.writeStatus()
	if p := f.status()["phase"]; p != "running" {
		t.Fatal(p)
	}
	os.WriteFile(f.w.pauseFile, nil, 0o600)
	f.writeStatus()
	if p := f.status()["phase"]; p != "paused" {
		t.Fatal(p)
	}
	// The keys the apps read (worker/desktop/ui/src/model.ts, worker/mac/src/Status.m).
	s := f.status()
	var keys []string
	for k := range s {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	if got := strings.Join(keys, " "); got != "alias contacts invite last npub paired paused phase pid requests started updated version work" {
		t.Fatalf("keys: %s", got)
	}
	// The app compares it with its own (Remove stale containers offers an older worker's container).
	if v := s["version"]; v != Version {
		t.Fatalf("version: %v", v)
	}
}

func TestWorkerRequestRecordsForTheOwner(t *testing.T) {
	needPOSIX(t)
	f := newWorkerFixture(t)
	f.exec("echo hi")
	rec := f.readJSON(filepath.Join(f.home, "requests", testRID+".json"))
	if rec["op"] != "exec" || rec["alias"] != "muse" || rec["cmd"] != "echo hi" || rec["decision"] != "allow" || rec["exit"] != 0.0 || rec["stdout"] != "hi\n" {
		t.Fatalf("%v", rec)
	}
	if _, ok := rec["duration"]; !ok || rec["done_at"].(float64) < rec["at"].(float64) {
		t.Fatalf("%v", rec)
	}
	var keys []string
	for k := range rec {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	if got := strings.Join(keys, " "); got != "alias at cmd decision done_at duration exit id op sender stderr stdout truncated" {
		t.Fatalf("keys: %s", got)
	}
	os.WriteFile(f.w.pauseFile, nil, 0o600)
	cd := strings.Repeat("cd", 16)
	f.w.handle(f.ctx, request(map[string]any{"myous": "exec", "id": cd, "cmd": "echo no"}))
	rec = f.readJSON(filepath.Join(f.home, "requests", cd+".json"))
	if rec["decision"] != "refuse" || rec["reason"] != pausedByOwner {
		t.Fatalf("%v", rec)
	}
	// Odd ids are refused, and can't name a record elsewhere.
	f.w.handle(f.ctx, request(map[string]any{"myous": "exec", "id": "../x", "cmd": "echo"}))
	entries, _ := os.ReadDir(filepath.Join(f.home, "requests"))
	var names []string
	for _, e := range entries {
		names = append(names, e.Name())
	}
	if got := strings.Join(names, " "); got != testRID+".json "+cd+".json" {
		t.Fatalf("records: %s", got)
	}
}

func TestWorkerRequestRecordsArePruned(t *testing.T) {
	f := newWorkerFixture(t)
	for i := 0; i < requestsKeep+5; i++ {
		f.w.record(fmt.Sprintf("r%04d", i), pyObj{{"op", "exec"}})
	}
	kept, _ := filepath.Glob(filepath.Join(f.home, "requests", "*.json"))
	if len(kept) != requestsKeep {
		t.Fatalf("%d kept", len(kept))
	}
	if exists(filepath.Join(f.home, "requests", "r0000.json")) {
		t.Fatal("the oldest is still there")
	}
}

func TestWorkerOwnerCommands(t *testing.T) {
	f := newWorkerFixture(t)
	f.agent.setContacts(map[string]Contact{})
	f.ensureInvite()
	if f.agent.invites != 1 {
		t.Fatal(f.agent.invites)
	}
	commands := filepath.Join(f.home, "commands")
	os.Mkdir(commands, 0o700)
	os.WriteFile(filepath.Join(commands, "new-code"), nil, 0o600)
	if err := f.w.tick(f.ctx); err != nil {
		t.Fatal(err)
	}
	if f.agent.invites != 2 || exists(filepath.Join(commands, "new-code")) {
		t.Fatalf("new-code: %d invites", f.agent.invites)
	}
	// Paired: new-code does nothing; unpair drops the contacts and re-invites.
	paired := map[string]Contact{"k": {Alias: "muse", Npub: testPeer, Status: Approved}}
	f.agent.setContacts(paired)
	f.st.Put("contacts", paired)
	f.w.paired = pyObj{{"alias", "muse"}, {"verify", "1"}, {"at", 1}}
	os.WriteFile(filepath.Join(commands, "new-code"), nil, 0o600)
	f.w.tick(f.ctx)
	if f.agent.invites != 2 {
		t.Fatal(f.agent.invites)
	}
	// An old stop file is cleared; a fresh one is left for the command it belongs to.
	old, fresh := filepath.Join(commands, "stop-old"), filepath.Join(commands, "stop-fresh")
	os.WriteFile(old, nil, 0o600)
	os.WriteFile(fresh, nil, 0o600)
	os.Chtimes(old, time.Now().Add(-time.Hour), time.Now().Add(-time.Hour))
	os.WriteFile(filepath.Join(commands, "unpair"), nil, 0o600)
	f.w.tick(f.ctx)
	var contacts map[string]Contact
	f.st.Get("contacts", &contacts)
	if len(contacts) != 0 || f.w.paired != nil {
		t.Fatalf("after unpair: %v %v", contacts, f.w.paired)
	}
	if exists(old) || !exists(fresh) {
		t.Fatal("stop files")
	}
	log := f.logLines()
	if l := log[len(log)-1]; l["op"] != "unpair" {
		t.Fatalf("%v", l)
	}
}

func TestWorkerOnWindowsSaysPowerShell(t *testing.T) {
	needPOSIX(t) // the command below is sh; the Windows behaviour is switched on by hand
	f := newWorkerFixture(t)
	f.w.windows = true
	f.w.handle(f.ctx, textEntry("help"))
	lines := strings.Split(f.lastText(), "\n")
	if lines[1] != windowsHelp || !strings.Contains(f.lastText(), "Shell: PowerShell on ") {
		t.Fatalf("help:\n%s", f.lastText())
	}
	if m := pairingMessage("1234-ABCDE", "PC", "", true); !strings.HasSuffix(m, "before using it. It runs on Windows and takes PowerShell commands.") {
		t.Fatal(m)
	}
	f.agent.setContacts(map[string]Contact{})
	f.ensureInvite()
	if m := f.status()["invite"].(map[string]any)["message"].(string); !strings.HasSuffix(m, "takes PowerShell commands.") {
		t.Fatal(m)
	}
	cmdletError := "ls : The term 'ls' is not recognized as the name of a cmdlet, function, script file, or operable program."
	r := f.exec(fmt.Sprintf("printf '%%s\\n' \"%s\" >&2; exit 1", cmdletError))
	if r["exit"] != 1.0 || r["stderr"] != cmdletError+"\n"+windowsHint {
		t.Fatalf("%v", r)
	}
	if rec := f.readJSON(filepath.Join(f.home, "requests", testRID+".json")); rec["stderr"] != r["stderr"] {
		t.Fatalf("record: %v", rec)
	}
	// Elsewhere, none of it.
	f.w.windows = false
	f.w.helpSent = map[string]time.Time{}
	f.w.handle(f.ctx, textEntry("help"))
	if strings.Contains(f.lastText(), windowsHelp) {
		t.Fatal(f.lastText())
	}
	if m := pairingMessage("1234-ABCDE", "PC", "", false); strings.Contains(m, "PowerShell") {
		t.Fatal(m)
	}
	if r := f.exec(fmt.Sprintf("printf '%%s\\n' \"%s\" >&2; exit 1", cmdletError)); r["stderr"] != cmdletError+"\n" {
		t.Fatalf("%v", r)
	}
}

func TestWorkerLongReplyIsRefused(t *testing.T) {
	needPOSIX(t)
	// Non-ASCII output is escaped (as Python's json.dumps does), so 150 KB
	// of it doesn't fit a message: the requester gets a refusal, as from
	// the Python worker.
	f := newWorkerFixture(t)
	r := f.exec(`i=0; while [ $i -lt 30000 ]; do printf '€€€€€'; i=$((i+1)); done`)
	if r["ok"] != false || !strings.HasPrefix(r["error"].(string), "bad request: message is ") {
		t.Fatalf("%v", r)
	}
}

func TestPyJSON(t *testing.T) {
	v := pyObj{{"exit", 3}, {"stdout", "out\n"}, {"stderr", "é😀\x7f\x01<>&/"}, {"truncated", false}, {"myous", "result"},
		{"id", "ab"}, {"duration", 0.01}, {"d2", 1.0}, {"d3", 1e-05}, {"d4", 1e16}, {"d5", 123456.789}, {"n", nil}}
	// json.dumps of the same dict, by Python.
	want := `{"exit": 3, "stdout": "out\n", "stderr": "\u00e9\ud83d\ude00\u007f\u0001<>&/", "truncated": false, "myous": "result", "id": "ab", "duration": 0.01, "d2": 1.0, "d3": 1e-05, "d4": 1e+16, "d5": 123456.789, "n": null}`
	if got := pyDumps(v, false); got != want {
		t.Fatalf("got  %s\nwant %s", got, want)
	}
	if got := pyDumps(map[string]any{"b": 1, "a": []any{1, "x"}}, true); got != `{"a": [1, "x"], "b": 1}` {
		t.Fatal(got)
	}
	for text, ok := range map[string]bool{
		`{"myous": "exec", "id": "x"}`: true, `{"myous": "exec", "id": 1}`: false, ` {"myous": "exec", "id": "x"}`: false,
		`{"myous": "exec", "id": "x"} extra`: false, `{"myous": "exec", "id": "x", "t": NaN, "u": -Infinity, "s": "NaN"}`: true,
		`["myous"]`: false, `{"myous": "exec", "id": "x"`: false,
	} {
		if _, got := parseRequest(text); got != ok {
			t.Errorf("parseRequest(%s) = %v", text, got)
		}
	}
	// NaN and ±Infinity are numbers, as json.loads reads them (a ttl of NaN is refused, a null one isn't).
	obj, _ := parseRequest(`{"myous": "exec", "id": "x", "s": "Infinity NaN", "t": Infinity, "u": -Infinity, "v": NaN, "w": 1e400, "n": null}`)
	tf, _ := pyNumber(obj["t"])
	uf, _ := pyNumber(obj["u"])
	vf, vok := pyNumber(obj["v"])
	if obj["s"] != "Infinity NaN" || !math.IsInf(tf, 1) || !math.IsInf(uf, -1) || !vok || !math.IsNaN(vf) || obj["n"] != nil {
		t.Fatalf("%v", obj)
	}
	if got := pyDumps(pyObj{{"t", obj["t"]}, {"u", obj["u"]}, {"v", obj["v"]}, {"w", obj["w"]}}, false); got != `{"t": Infinity, "u": -Infinity, "v": NaN, "w": Infinity}` {
		t.Fatal(got)
	}
	// bytes.decode("utf-8", "replace"), by Python: one U+FFFD per maximal invalid sequence.
	for in, want := range map[string]string{
		"\xe2\x82": "\ufffd", "\xed\xa0\x80": "\ufffd\ufffd\ufffd", "\xe2\x28\xa1": "\ufffd(\ufffd", "\xf0\x9f\x98": "\ufffd",
		"\xff\xfe": "\ufffd\ufffd", "\xc0\xaf": "\ufffd\ufffd", "\xf4\x90\x80\x80": "\ufffd\ufffd\ufffd\ufffd",
		"a\xf0\x9f\x98\x80b": "a😀b", "\xe0\x80\x80": "\ufffd\ufffd\ufffd", "\xf8\x88\x80\x80\x80": "\ufffd\ufffd\ufffd\ufffd\ufffd", "\xc3": "\ufffd",
	} {
		if got := pyDecode([]byte(in)); got != want {
			t.Errorf("pyDecode(%q) = %q, want %q", in, got, want)
		}
	}
}

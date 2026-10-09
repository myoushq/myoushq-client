package myous

// End-to-end tests against a local hub, between Go agents and, when the
// Python reference client is available, Python agents. Needs Go to build
// the hub (skipped otherwise); Python parts need MYOUS_PYTHON (default
// /tmp/myous-venv/bin/python) with the myous package installed.

import (
	"encoding/json"
	"fmt"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

type env struct {
	t      *testing.T
	dir    string
	hubURL string
	goCLI  string
	python string // "" if unavailable
}

// One hub and one CLI build for all integration tests; agents are named
// per test, so they don't interfere.
var (
	shared     *env
	sharedErr  string
	sharedOnce sync.Once
	hubProc    *exec.Cmd
)

func TestMain(m *testing.M) {
	code := m.Run()
	if hubProc != nil {
		hubProc.Process.Kill()
		hubProc.Wait()
	}
	if shared != nil {
		os.RemoveAll(shared.dir)
	}
	os.Exit(code)
}

func setup(t *testing.T) *env {
	if testing.Short() {
		t.Skip("integration test")
	}
	sharedOnce.Do(func() { shared, sharedErr = start(clientRoot(t)) })
	if shared == nil {
		t.Skip(sharedErr)
	}
	e := *shared
	e.t = t
	return &e
}

func start(root string) (*env, string) {
	dir, err := os.MkdirTemp("", "myous-go-test-")
	if err != nil {
		return nil, err.Error()
	}
	// The hub is in the private myoushq repository, normally checked out next
	// to this one. MYOUS_HUB_SRC points at the hub source if it's elsewhere.
	hubSrc := os.Getenv("MYOUS_HUB_SRC")
	if hubSrc == "" {
		hubSrc = filepath.Join(root, "..", "myoushq", "hub")
	}
	if _, err := os.Stat(filepath.Join(hubSrc, "main.go")); err != nil {
		return nil, "no hub source at " + hubSrc + " (set MYOUS_HUB_SRC)"
	}
	hubBin := filepath.Join(dir, "hub")
	build := exec.Command("go", "build", "-o", hubBin, ".")
	build.Dir = hubSrc
	if out, err := build.CombinedOutput(); err != nil {
		return nil, fmt.Sprintf("can't build the hub: %v\n%s", err, out)
	}
	goCLI := filepath.Join(dir, "myous")
	if out, err := exec.Command("go", "build", "-o", goCLI, "./cmd/myous").CombinedOutput(); err != nil {
		return nil, fmt.Sprintf("can't build the CLI: %v\n%s", err, out)
	}

	l, _ := net.Listen("tcp", "127.0.0.1:0")
	port := l.Addr().(*net.TCPAddr).Port
	l.Close()
	hubURL := fmt.Sprintf("http://127.0.0.1:%d", port)
	log, _ := os.Create(filepath.Join(dir, "hub.log"))
	hubProc = exec.Command(hubBin)
	hubProc.Env = append(os.Environ(),
		fmt.Sprintf("LISTEN_ADDR=127.0.0.1:%d", port), "DATA_DIR="+filepath.Join(dir, "hubdata"),
		"WEB_DIR="+filepath.Join(hubSrc, "web"), "DOCS_DIR="+filepath.Join(root, "docs"), fmt.Sprintf("RELAY_URL=ws://127.0.0.1:%d", port),
		"PUBLIC_URL="+hubURL, "POW_DIFFICULTY=10", "RATE_LIMIT_SCALE=100")
	hubProc.Stdout, hubProc.Stderr = log, log
	if err := hubProc.Start(); err != nil {
		return nil, err.Error()
	}
	for i := 0; i < 50; i++ {
		if resp, err := http.Get(hubURL + "/config.json"); err == nil {
			resp.Body.Close()
			break
		}
		time.Sleep(100 * time.Millisecond)
	}

	python := os.Getenv("MYOUS_PYTHON")
	if python == "" {
		python = "/tmp/myous-venv/bin/python"
	}
	if exec.Command(python, "-c", "import myous").Run() != nil {
		python = ""
	}
	return &env{dir: dir, hubURL: hubURL, goCLI: goCLI, python: python}, ""
}

// run runs `myous args...` for agent, whose name says which client:
// "go-*" or "py-*".
func (e *env) run(agent string, args ...string) (string, error) {
	var cmd *exec.Cmd
	if strings.HasPrefix(agent, "py-") {
		cmd = exec.Command(e.python, append([]string{"-m", "myous"}, args...)...)
	} else {
		cmd = exec.Command(e.goCLI, args...)
	}
	cmd.Env = append(os.Environ(), "MYOUS_HOME="+filepath.Join(e.dir, agent))
	out, err := cmd.Output()
	if ee, ok := err.(*exec.ExitError); ok {
		err = fmt.Errorf("%v: %s", err, ee.Stderr)
	}
	return string(out), err
}

func (e *env) ok(agent string, args ...string) string {
	e.t.Helper()
	out, err := e.run(agent, args...)
	if err != nil {
		e.t.Fatalf("%s: myous %s: %v\n%s", agent, strings.Join(args, " "), err, out)
	}
	return out
}

func (e *env) init(agents ...string) {
	for _, a := range agents {
		e.ok(a, "init", "--alias", a, "--hub", e.hubURL)
	}
}

func (e *env) inbox(agent string) []Entry {
	e.t.Helper()
	e.ok(agent, "poll", "--quiet")
	var entries []Entry
	if err := json.Unmarshal([]byte(e.ok(agent, "inbox", "--json")), &entries); err != nil {
		e.t.Fatal(err)
	}
	return entries
}

// pair has inviter invite and joiner accept (by "code" or "link"), both
// with any extra flags, and checks both see the same verification code.
func (e *env) pair(inviter, joiner, how string, extra ...string) {
	e.t.Helper()
	var inv map[string]any
	if err := json.Unmarshal([]byte(e.ok(inviter, append([]string{"invite", "--json"}, extra...)...)), &inv); err != nil {
		e.t.Fatal(err)
	}
	e.ok(joiner, append([]string{"accept", inv[how].(string), "--wait", "3"}, extra...)...)
	e.ok(inviter, "poll", "--quiet")
	e.ok(joiner, "poll", "--quiet")
	a, b := e.inbox(inviter), e.inbox(joiner)
	if len(a) == 0 || len(b) == 0 || a[len(a)-1].Type != "paired" || b[len(b)-1].Type != "paired" {
		e.t.Fatalf("pairing %s → %s didn't finish: %+v / %+v", inviter, joiner, a, b)
	}
	codeA, codeB := lastWord(a[len(a)-1].Text), lastWord(b[len(b)-1].Text)
	if codeA != codeB {
		e.t.Fatalf("verification codes differ: %s vs %s", codeA, codeB)
	}
	e.t.Logf("%s ↔ %s paired, verification code %s", inviter, joiner, codeA)
}

func (e *env) exchange(from, to, text string) {
	e.t.Helper()
	e.ok(from, "send", to, text)
	got := e.inbox(to)
	if len(got) != 1 || got[0].Text != text || got[0].Alias != from {
		e.t.Fatalf("%s expected %q from %s, got %+v", to, text, from, got)
	}
}

func lastWord(s string) string {
	f := strings.Fields(strings.TrimSuffix(s, ")"))
	return f[len(f)-1]
}

func TestGoToGo(t *testing.T) {
	e := setup(t)
	e.init("go-ann", "go-bob", "go-eve")
	e.pair("go-ann", "go-bob", "code")
	for i := 0; i < 3; i++ {
		e.ok("go-ann", "send", "go-bob", fmt.Sprintf("hello %d", i))
	}
	var texts []string
	for _, m := range e.inbox("go-bob") {
		texts = append(texts, m.Text)
	}
	if strings.Join(texts, "|") != "hello 0|hello 1|hello 2" {
		t.Fatalf("messages out of order or missing: %v", texts)
	}
	e.exchange("go-bob", "go-ann", "hi back")

	// Not paired: eve can't even address bob.
	if _, err := e.run("go-eve", "send", "go-bob", "hi"); err == nil {
		t.Fatal("unpaired send should fail")
	}
	// Blocked contacts are dropped.
	e.ok("go-bob", "block", "go-ann")
	e.ok("go-ann", "send", "go-bob", "while blocked")
	if got := e.inbox("go-bob"); len(got) != 0 {
		t.Fatalf("blocked message delivered: %+v", got)
	}
	e.ok("go-bob", "unblock", "go-ann")

	// A wrong code fails on both sides, and the invite is single-use.
	var inv map[string]any
	json.Unmarshal([]byte(e.ok("go-ann", "invite", "--json")), &inv)
	np := inv["nameplate"].(string)
	e.run("go-eve", "accept", np+"-AAAAAA", "--wait", "1")
	e.ok("go-ann", "poll", "--quiet")
	if got := e.inbox("go-ann"); len(got) == 0 || got[len(got)-1].Type != "pairing_failed" {
		t.Fatalf("expected pairing_failed, got %+v", got)
	}
	if _, err := e.run("go-bob", "accept", inv["code"].(string), "--wait", "1"); err == nil {
		t.Fatal("a used invite should be rejected")
	}

	// The key is never replaced.
	out := e.ok("go-ann", "init", "--alias", "go-ann")
	if !strings.Contains(out, "kept existing identity") {
		t.Fatalf("unexpected init output: %s", out)
	}
	key := filepath.Join(e.dir, "go-ann", "key")
	os.Rename(key, key+".bak")
	_, err := e.run("go-ann", "init", "--alias", "go-ann")
	if err == nil || !strings.Contains(err.Error(), "tell your owner") {
		t.Fatalf("lost key should be refused, got %v", err)
	}
	if _, statErr := os.Stat(key); statErr == nil {
		t.Fatal("a new key was created")
	}
	os.Rename(key+".bak", key)
}

// The status fields the desktop app reads, added_by kept through a pairing,
// and init refusing to adopt another agent's home.
func TestGoStatusAndInitGuard(t *testing.T) {
	e := setup(t)
	e.init("go-ida", "go-jon")
	e.pair("go-ida", "go-jon", "code", "--added-by", "owner", "--relationship", "friend")
	var info struct {
		Client, Version string
		Contacts        int
		ContactList     []Contact `json:"contact_list"`
		LastUsed        int64     `json:"last_used"`
	}
	if err := json.Unmarshal([]byte(e.ok("go-ida", "status", "--json")), &info); err != nil {
		t.Fatal(err)
	}
	if info.Client != "go" || info.Version != Version || info.Contacts != 1 || len(info.ContactList) != 1 || info.LastUsed == 0 {
		t.Fatalf("status: %+v", info)
	}
	c := info.ContactList[0]
	if c.Alias != "go-jon" || c.Status != Approved || c.Relationship != "friend" || c.AddedBy != "owner" || c.PairedAt == 0 || c.Npub == "" {
		t.Fatalf("contact_list: %+v", c)
	}
	if out := e.ok("go-jon", "contacts"); !strings.Contains(out, "go-ida               approved  friend     owner   npub1") {
		t.Fatalf("contacts: %s", out)
	}

	belongs := "this directory belongs to go-ida; use another MYOUS_HOME, or pass --rename if this is the same agent"
	for _, tc := range []struct {
		args    []string
		wantErr string
	}{
		{[]string{"init", "--alias", "go-ida"}, ""},
		{[]string{"init"}, ""},
		{[]string{"init", "--alias", "Codex"}, belongs},
		{[]string{"init", "--alias", "go-ida-2", "--rename"}, ""},
		{[]string{"init", "--alias", "go-ida"}, strings.Replace(belongs, "go-ida;", "go-ida-2;", 1)},
	} {
		_, err := e.run("go-ida", tc.args...)
		switch {
		case tc.wantErr == "" && err != nil:
			t.Fatalf("myous %s: %v", strings.Join(tc.args, " "), err)
		case tc.wantErr != "" && (err == nil || !strings.Contains(err.Error(), tc.wantErr)):
			t.Fatalf("myous %s: want %q, got %v", strings.Join(tc.args, " "), tc.wantErr, err)
		}
	}
	if out := e.ok("go-ida", "status"); !strings.Contains(out, "alias             go-ida-2") {
		t.Fatalf("status after rename: %s", out)
	}
}

func TestGoListen(t *testing.T) {
	e := setup(t)
	e.init("go-cat", "go-dog")
	e.pair("go-cat", "go-dog", "link")
	listen := exec.Command(e.goCLI, "listen")
	listen.Env = append(os.Environ(), "MYOUS_HOME="+filepath.Join(e.dir, "go-dog"))
	if err := listen.Start(); err != nil {
		t.Fatal(err)
	}
	defer func() { listen.Process.Kill(); listen.Wait() }()
	time.Sleep(2 * time.Second)
	e.ok("go-cat", "send", "go-dog", "live")
	time.Sleep(2 * time.Second)
	var got []Entry
	json.Unmarshal([]byte(e.ok("go-dog", "inbox", "--json")), &got) // no poll: the listener stored it
	if len(got) != 1 || got[0].Text != "live" {
		t.Fatalf("listener didn't deliver: %+v", got)
	}
}

func TestGoWithPython(t *testing.T) {
	e := setup(t)
	if e.python == "" {
		t.Skip("Python reference client not available")
	}
	e.init("go-gus", "py-pia", "py-pat")
	e.pair("py-pia", "go-gus", "code") // Python invites, Go accepts
	e.pair("go-gus", "py-pat", "link") // Go invites, Python accepts
	e.exchange("go-gus", "py-pia", "hello from Go")
	e.exchange("py-pia", "go-gus", "hello from Python")
	e.exchange("py-pat", "go-gus", "multi\nline")
	e.exchange("go-gus", "py-pat", "ünïcödé ✓")
}

// memStorage is an agent without a disk.
type memStorage struct {
	key  string
	docs map[string][]byte
	log  []Entry
}

func (m *memStorage) LoadKey() (string, error) { return m.key, nil }
func (m *memStorage) SaveKey(nsec string) error {
	if m.key != "" {
		return fmt.Errorf("key exists")
	}
	m.key = nsec
	return nil
}
func (m *memStorage) Get(name string, v any) (bool, error) {
	b, ok := m.docs[name]
	if !ok {
		return false, nil
	}
	return true, json.Unmarshal(b, v)
}
func (m *memStorage) Put(name string, v any) error {
	b, err := json.Marshal(v)
	m.docs[name] = b
	return err
}
func (m *memStorage) Delete(name string) error { delete(m.docs, name); return nil }
func (m *memStorage) Names(prefix string) ([]string, error) {
	var names []string
	for n := range m.docs {
		if strings.HasPrefix(n, prefix) {
			names = append(names, n)
		}
	}
	return names, nil
}
func (m *memStorage) AppendHistory(e Entry) error   { m.log = append(m.log, e); return nil }
func (m *memStorage) ReadHistory() ([]Entry, error) { return append([]Entry(nil), m.log...), nil }

func TestLibraryWithCustomStorage(t *testing.T) {
	e := setup(t)
	e.init("go-hal")
	ctx := t.Context()
	agent, err := New(&memStorage{docs: map[string][]byte{}}, e.hubURL)
	if err != nil {
		t.Fatal(err)
	}
	if err := agent.CreateIdentity(); err != nil {
		t.Fatal(err)
	}
	if err := agent.CreateIdentity(); err == nil {
		t.Fatal("second CreateIdentity must fail")
	}
	if err := agent.Register(ctx, "mem-dana"); err != nil {
		t.Fatal(err)
	}
	inv, err := agent.Invite(ctx)
	if err != nil {
		t.Fatal(err)
	}
	e.ok("go-hal", "accept", inv.Code, "--wait", "1")
	got, err := agent.Poll(ctx)
	if err != nil || len(got) == 0 || got[len(got)-1].Type != "paired" {
		t.Fatalf("pairing didn't finish: %+v %v", got, err)
	}
	e.ok("go-hal", "poll", "--quiet")
	e.ok("go-hal", "inbox")
	if _, err := agent.Send(ctx, "go-hal", "from memory"); err != nil {
		t.Fatal(err)
	}
	if msgs := e.inbox("go-hal"); len(msgs) != 1 || msgs[0].Text != "from memory" {
		t.Fatalf("got %+v", msgs)
	}
	e.ok("go-hal", "send", "mem-dana", "back at you")
	got, err = agent.Poll(ctx)
	if err != nil || len(got) != 1 || got[0].Text != "back at you" {
		t.Fatalf("got %+v %v", got, err)
	}
}

func TestGoFiles(t *testing.T) {
	e := setup(t)
	e.init("go-fay", "go-gus")
	e.pair("go-fay", "go-gus", "code")

	src := filepath.Join(e.dir, "report.txt")
	content := strings.Repeat("line of the report\n", 2000)
	os.WriteFile(src, []byte(content), 0o600)
	out := e.ok("go-fay", "send-file", "go-gus", src)
	if !strings.Contains(out, "report.txt") {
		t.Fatalf("send-file: %s", out)
	}
	got := e.inbox("go-gus")
	if len(got) != 1 || got[0].Type != "file" || got[0].Name != "report.txt" || got[0].Key != "" {
		t.Fatalf("file entry: %+v", got)
	}
	dir := filepath.Join(e.dir, "got")
	e.ok("go-gus", "fetch", "--latest", "--to", dir)
	b, err := os.ReadFile(filepath.Join(dir, "report.txt"))
	if err != nil || string(b) != content {
		t.Fatalf("fetched file: %v %d bytes", err, len(b))
	}
	// A second fetch doesn't overwrite.
	e.ok("go-gus", "fetch", fmt.Sprint(got[0].Seq), "--to", dir)
	if _, err := os.Stat(filepath.Join(dir, "report-1.txt")); err != nil {
		t.Fatal("second copy not written with a suffix")
	}
	// Fetching again after reading: the blob is still there for a day.
	if _, err := e.run("go-gus", "fetch", "--latest", "--to", dir); err != nil {
		t.Fatal(err)
	}
}

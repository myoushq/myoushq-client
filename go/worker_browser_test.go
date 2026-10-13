package myous

// The browser announcement: when browser.json names a new browser, the
// worker tells its approved contacts once (worker_browser.go).

import (
	"bytes"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// newBrowser writes browser.json with a fresh mtime (as the app does at
// each browser start), so a rewrite within the file system's mtime
// granularity still counts as a change.
func (f *workerFixture) newBrowser(fields string) {
	f.t.Helper()
	f.writeBrowserJSON(fields)
	f.mtime = f.mtime.Add(time.Second)
	if f.mtime.Before(time.Now()) {
		f.mtime = time.Now()
	}
	path := filepath.Join(f.home, "browser.json")
	if err := os.Chtimes(path, f.mtime, f.mtime); err != nil {
		f.t.Fatal(err)
	}
}

func (f *workerFixture) tick() {
	f.t.Helper()
	if err := f.w.tick(f.ctx); err != nil {
		f.t.Fatal(err)
	}
}

func (f *workerFixture) toldFile() string {
	raw, err := os.ReadFile(filepath.Join(f.home, browserToldFile))
	if err != nil {
		return ""
	}
	return string(raw)
}

func TestWorkerAnnouncesItsBrowser(t *testing.T) {
	needPOSIX(t) // the stand-in Python is a shell script; Windows is switched on by hand
	f := newWorkerFixture(t)
	f.w.windows = true
	f.w.python = pythonInfo{version: "3.12.1", at: time.Now()} // the plain one, without Playwright
	// No browser: nothing to say.
	f.tick()
	if n := f.agent.sentCount(); n != 0 {
		t.Fatalf("%d sent without a browser", n)
	}

	py := fakePython(t, filepath.Join(f.tmp, "browser-python", "Scripts"), "3.12.10")
	browserJSON := func(port string) string {
		return `{"host": "127.0.0.1", "port": ` + port + `, "pid": 777, "app": "Edge", "at": 1760000000, "python": ` +
			jsonString(py) + `, "kind": "host"}`
	}
	f.newBrowser(browserJSON("41234"))
	f.tick()
	if n := f.agent.sentCount(); n != 1 {
		t.Fatalf("%d sent, want 1", n)
	}
	f.agent.mu.Lock()
	to, text := f.agent.sent[0][0], f.agent.sent[0][1]
	f.agent.mu.Unlock()
	if to != testPeer {
		t.Fatalf("sent to %s", to)
	}
	want := "The worker's browser is ready.\n" +
		"Browser: a Chromium window on this computer (Edge), CDP at http://127.0.0.1:41234 " +
		"(Playwright: chromium.connect_over_cdp, then browser.contexts[0], which has the logins). " +
		"Its owner logs into sites there; ask them to rather than logging in yourself.\n" +
		`Python with Playwright: & "` + py + `" script.py (3.12.10; the plain python on this computer has no Playwright)` + "\n" +
		"Send help for the rest."
	if text != want {
		t.Fatalf("announcement:\n%s\nwant:\n%s", text, want)
	}
	// The same lines as the help's.
	help := f.freshHelp()
	shell := helpLine(help, "Shell: ")
	fromHelp := "The worker's browser is ready.\n" + helpLine(help, "Browser: a Chromium") + "\n" +
		shell[strings.Index(shell, "Python with Playwright: "):] + "\nSend help for the rest."
	if text != fromHelp {
		t.Fatalf("announcement:\n%s\nhelp's lines:\n%s", text, fromHelp)
	}
	if got, want := f.toldFile(), `{"port":41234,"python":`+jsonString(py)+`}`; got != want {
		t.Fatalf("%s: %s, want %s", browserToldFile, got, want)
	}

	// Once: not on the next tick, nor when the app rewrites the same browser.
	sent := f.agent.sentCount()
	f.tick()
	f.newBrowser(browserJSON("41234"))
	f.tick()
	if n := f.agent.sentCount(); n != sent {
		t.Fatalf("announced again: %s", f.lastText())
	}
	// The browser ends: nothing said. It starts again on another port: told.
	os.Remove(filepath.Join(f.home, "browser.json"))
	f.tick()
	if n := f.agent.sentCount(); n != sent {
		t.Fatalf("said something when the browser ended: %s", f.lastText())
	}
	f.newBrowser(browserJSON("41300"))
	f.tick()
	if n := f.agent.sentCount(); n != sent+1 || !strings.Contains(f.lastText(), "CDP at http://127.0.0.1:41300 ") {
		t.Fatalf("%d sent, last %q", n-sent, f.lastText())
	}
	// A new port without the file going away first: told too.
	f.newBrowser(browserJSON("41301"))
	f.tick()
	if n := f.agent.sentCount(); n != sent+2 || !strings.Contains(f.lastText(), "CDP at http://127.0.0.1:41301 ") {
		t.Fatalf("%d sent, last %q", n-sent, f.lastText())
	}
	if !strings.Contains(f.toldFile(), `"port":41301`) {
		t.Fatal(f.toldFile())
	}
}

func TestWorkerAnnouncesToApprovedContactsOnly(t *testing.T) {
	f := newWorkerFixture(t)
	f.agent.setContacts(map[string]Contact{
		"k1": {Alias: "muse", Npub: testPeer, Status: Approved},
		"k2": {Alias: "old", Npub: "npub1blocked", Status: Blocked},
		"k3": {Alias: "other", Npub: "npub1other", Status: Approved},
	})
	f.newBrowser(`{"port": 41234, "app": "Edge", "kind": "host"}`)
	f.tick()
	f.agent.mu.Lock()
	sent := append([][2]string(nil), f.agent.sent...)
	f.agent.mu.Unlock()
	if len(sent) != 2 || sent[0][0] != "npub1other" || sent[1][0] != testPeer {
		t.Fatalf("sent: %v", sent)
	}
	// No Python named: no Python line.
	want := "The worker's browser is ready.\n" + browserLine(browserInfo{host: "127.0.0.1", port: 41234, app: "Edge"}) +
		"\nSend help for the rest."
	if sent[0][1] != want {
		t.Fatalf("announcement:\n%s", sent[0][1])
	}
}

func TestWorkerAnnouncesNothingWithoutContacts(t *testing.T) {
	f := newWorkerFixture(t)
	f.agent.setContacts(map[string]Contact{"k": {Alias: "muse", Npub: testPeer, Status: Blocked}})
	f.newBrowser(`{"port": 41234, "app": "Edge", "kind": "host"}`)
	f.tick()
	if n := f.agent.sentCount(); n != 0 {
		t.Fatalf("%d sent without approved contacts", n)
	}
	// Recorded all the same: a contact paired later gets the browser from
	// the help, not from an announcement at the worker's next start.
	if got := f.toldFile(); got != `{"port":41234,"python":""}` {
		t.Fatal(got)
	}
	f.agent.setContacts(map[string]Contact{"k": {Alias: "muse", Npub: testPeer, Status: Approved}})
	f.tick()
	if n := f.agent.sentCount(); n != 0 {
		t.Fatalf("%d sent", n)
	}
}

func TestWorkerStartedWithTheToldBrowserIsSilent(t *testing.T) {
	f := newWorkerFixture(t)
	f.newBrowser(`{"port": 41234, "app": "Edge", "python": "C:\\py\\python.exe", "kind": "host"}`)
	f.w.windows = true
	f.w.python = pythonInfo{at: time.Now()}
	f.w.browserPy = browserPython{path: `C:\py\python.exe`, version: "3.12.10", at: time.Now()}
	if err := os.WriteFile(filepath.Join(f.home, browserToldFile), []byte(`{"port": 41234, "python": "C:\\py\\python.exe"}`), 0o644); err != nil {
		t.Fatal(err)
	}
	f.tick()
	if n := f.agent.sentCount(); n != 0 {
		t.Fatalf("announced a browser already told: %s", f.lastText())
	}
	// Told about another Python (the app's Python was replaced): announced.
	if err := os.WriteFile(filepath.Join(f.home, browserToldFile), []byte(`{"port": 41234, "python": "C:\\old\\python.exe"}`), 0o644); err != nil {
		t.Fatal(err)
	}
	f2, err := newWorker(f.agent, f.st, WorkerOptions{Work: f.work, Out: &bytes.Buffer{}})
	if err != nil {
		t.Fatal(err)
	}
	f2.windows = true
	f2.python = f.w.python
	f2.browserPy = f.w.browserPy
	if err := f2.tick(f.ctx); err != nil {
		t.Fatal(err)
	}
	if n := f.agent.sentCount(); n != 1 || !strings.Contains(f.lastText(), `Python with Playwright: & "C:\py\python.exe" script.py (3.12.10)`) {
		t.Fatalf("%d sent: %q", n, f.lastText())
	}
}

func TestWorkerRetriesAFailedAnnouncement(t *testing.T) {
	f := newWorkerFixture(t)
	var out bytes.Buffer
	f.w.out = &out
	f.agent.setContacts(map[string]Contact{
		"k1": {Alias: "muse", Npub: testPeer, Status: Approved},
		"k2": {Alias: "other", Npub: "npub1other", Status: Approved},
	})
	f.agent.mu.Lock()
	f.agent.failSend = map[string]error{"npub1other": errors.New("relay unreachable")}
	f.agent.mu.Unlock()
	f.newBrowser(`{"port": 41234, "app": "Edge", "kind": "host"}`)
	f.tick()
	f.tick()
	// One failure doesn't stop the others; it is logged once and tried
	// again each tick; the told file waits for everyone.
	if n := f.agent.sentCount(); n != 1 || f.agent.sent[0][0] != testPeer {
		t.Fatalf("sent: %v", f.agent.sent)
	}
	if c := strings.Count(out.String(), "could not tell other the browser is ready"); c != 1 ||
		!strings.Contains(out.String(), "relay unreachable") {
		t.Fatalf("log:\n%s", out.String())
	}
	if got := f.toldFile(); got != "" {
		t.Fatalf("%s written before everyone was told: %s", browserToldFile, got)
	}
	f.agent.mu.Lock()
	f.agent.failSend = nil
	f.agent.mu.Unlock()
	f.tick()
	f.agent.mu.Lock()
	sent := append([][2]string(nil), f.agent.sent...)
	f.agent.mu.Unlock()
	if len(sent) != 2 || sent[1][0] != "npub1other" {
		t.Fatalf("sent: %v", sent)
	}
	if got := f.toldFile(); got != `{"port":41234,"python":""}` {
		t.Fatal(got)
	}
	f.tick()
	if n := f.agent.sentCount(); n != 2 {
		t.Fatalf("%d sent", n)
	}
}

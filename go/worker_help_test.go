package myous

// The worker's help and its browser: browser.json in the worker's home,
// written by myous (the desktop app) while the worker's browser runs.

import (
	"fmt"
	"net"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// helpLine is the line of the help that starts with prefix ("" if none).
func helpLine(help, prefix string) string {
	for _, l := range strings.Split(help, "\n") {
		if strings.HasPrefix(l, prefix) {
			return l
		}
	}
	return ""
}

// freshHelp is the help as a contact who hasn't had one lately gets it.
func (f *workerFixture) freshHelp() string {
	f.w.helpSent = map[string]time.Time{}
	f.w.handle(f.ctx, textEntry("help"))
	return f.lastText()
}

// fakePython is a program that answers the version probe like a Python.
func fakePython(t *testing.T, dir, version string) string {
	t.Helper()
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	p := filepath.Join(dir, "python")
	if err := os.WriteFile(p, []byte("#!/bin/sh\necho "+version+"\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	return p
}

func (f *workerFixture) writeBrowserJSON(fields string) {
	f.t.Helper()
	if err := os.WriteFile(filepath.Join(f.home, "browser.json"), []byte(fields), 0o644); err != nil {
		f.t.Fatal(err)
	}
}

func jsonString(s string) string { return fmt.Sprintf("%q", s) }

func TestWorkerHelpWithoutABrowser(t *testing.T) {
	f := newWorkerFixture(t)
	f.w.python = pythonInfo{version: "3.12.1", at: time.Now()}
	help := f.freshHelp()
	if l := helpLine(help, "Shell: "); !strings.HasSuffix(l, "; Python 3.12.1 (no Playwright)") {
		t.Fatalf("shell line: %q", l)
	}
	if strings.Contains(help, "Browser: a Chromium window") || strings.Contains(help, "Python with Playwright") {
		t.Fatal(help)
	}
	f.w.python = pythonInfo{version: "3.12.1", playwright: true, at: time.Now()}
	if l := helpLine(f.freshHelp(), "Shell: "); !strings.HasSuffix(l, "; Python 3.12.1; Playwright installed") {
		t.Fatalf("shell line: %q", l)
	}
	// myous for Mac's browser.json (for the container's forward.py) has no
	// kind: not this worker's browser.
	f.writeBrowserJSON(`{"host": "host.docker.internal", "port": 9333, "pid": 1, "app": "Chrome", "at": 1}`)
	if help := f.freshHelp(); strings.Contains(help, "Browser: a Chromium window") {
		t.Fatal(help)
	}
	// Nor is one that isn't JSON or has no port.
	f.writeBrowserJSON(`{"kind": "host"`)
	if help := f.freshHelp(); strings.Contains(help, "Browser: a Chromium window") {
		t.Fatal(help)
	}
	f.writeBrowserJSON(`{"kind": "host", "host": "127.0.0.1"}`)
	if help := f.freshHelp(); strings.Contains(help, "Browser: a Chromium window") {
		t.Fatal(help)
	}
}

func TestWorkerHelpNamesItsBrowserOnWindows(t *testing.T) {
	needPOSIX(t) // the stand-in Python is a shell script; Windows is switched on by hand
	f := newWorkerFixture(t)
	f.w.windows = true
	f.w.python = pythonInfo{version: "3.12.1", at: time.Now()} // the plain one, without Playwright
	// No browser yet: the help says nothing of one.
	if help := f.freshHelp(); strings.Contains(help, "Browser: a Chromium window") {
		t.Fatal(help)
	}
	// The browser starts while the worker runs: the next help has it.
	py := fakePython(t, filepath.Join(f.tmp, "browser-python", "Scripts"), "3.12.10")
	f.writeBrowserJSON(`{"host": "127.0.0.1", "port": 41234, "pid": 777, "app": "Edge", "at": 1760000000, "python": ` +
		jsonString(py) + `, "kind": "host"}`)
	help := f.freshHelp()
	want := `; Python with Playwright: & "` + py + `" script.py (3.12.10; the plain python on this computer has no Playwright)`
	if l := helpLine(help, "Shell: PowerShell on "); !strings.HasSuffix(l, want) {
		t.Fatalf("shell line: %q\nwant suffix %q", l, want)
	}
	browser := "Browser: a Chromium window on this computer (Edge), CDP at http://127.0.0.1:41234 " +
		"(Playwright: chromium.connect_over_cdp, then browser.contexts[0], which has the logins). " +
		"Its owner logs into sites there; ask them to rather than logging in yourself."
	if helpLine(help, "Browser: a Chromium") != browser {
		t.Fatalf("help:\n%s", help)
	}
	// The --note lines still come last.
	if lines := strings.Split(help, "\n"); lines[len(lines)-1] != "Browser: http://localhost:9222" {
		t.Fatalf("help:\n%s", help)
	}
	// With Playwright in the plain Python too, nothing to warn about.
	f.w.python = pythonInfo{version: "3.12.1", playwright: true, at: time.Now()}
	if l := helpLine(f.freshHelp(), "Shell: "); !strings.HasSuffix(l, `" script.py (3.12.10)`) {
		t.Fatalf("shell line: %q", l)
	}
	// The browser ended (the app removed the file): gone from the help.
	os.Remove(filepath.Join(f.home, "browser.json"))
	if help := f.freshHelp(); strings.Contains(help, "Browser: a Chromium window") || strings.Contains(help, "Python with Playwright") {
		t.Fatal(help)
	}

	// PowerShell quoting: double quotes, single when $ ` or " would expand.
	for path, want := range map[string]string{
		`C:\Users\max\AppData\Local\myous\browser-python\Scripts\python.exe`: `& "C:\Users\max\AppData\Local\myous\browser-python\Scripts\python.exe"`,
		`C:\Users\a$b\python.exe`: `& 'C:\Users\a$b\python.exe'`,
		`C:\Users\o'neil$\py.exe`: `& 'C:\Users\o''neil$\py.exe'`,
		`C:\Program Files\py.exe`: `& "C:\Program Files\py.exe"`,
	} {
		if got := f.w.programCall(path); got != want {
			t.Errorf("%s: %s, want %s", path, got, want)
		}
	}
}

func TestWorkerHelpNamesItsBrowserElsewhere(t *testing.T) {
	needPOSIX(t)
	f := newWorkerFixture(t)
	f.w.windows = false
	f.w.python = pythonInfo{version: "3.11.2", at: time.Now()}
	py := fakePython(t, filepath.Join(f.tmp, "o'neil x", "bin"), "3.12.3")
	f.writeBrowserJSON(`{"host": "127.0.0.1", "port": 41235, "pid": 1, "app": "Chrome", "python": ` + jsonString(py) + `, "kind": "host"}`)
	help := f.freshHelp()
	quoted := "'" + strings.ReplaceAll(py, "'", `'\''`) + "'"
	want := "; Python with Playwright: " + quoted + " script.py (3.12.3; the plain python on this computer has no Playwright)"
	if l := helpLine(help, "Shell: "); !strings.HasSuffix(l, want) {
		t.Fatalf("shell line: %q\nwant suffix %q", l, want)
	}
	if !strings.Contains(helpLine(help, "Browser: a Chromium"), "(Chrome), CDP at http://127.0.0.1:41235 ") {
		t.Fatal(help)
	}
	for path, want := range map[string]string{
		"/home/max/.local/share/myous/browser-python/bin/python": "/home/max/.local/share/myous/browser-python/bin/python",
		"/home/o'neil/x y/python":                                `'/home/o'\''neil/x y/python'`,
		"":                                                       "''",
	} {
		if got := f.w.programCall(path); got != want {
			t.Errorf("%q: %s, want %s", path, got, want)
		}
	}
	// A Python that doesn't run: no version, the rest as before; and
	// without a plain Python, nothing about it.
	f.w.python = pythonInfo{at: time.Now()}
	f.writeBrowserJSON(`{"port": 41235, "python": "/nonexistent/python", "kind": "host"}`)
	if l := helpLine(f.freshHelp(), "Shell: "); !strings.HasSuffix(l, "; Python with Playwright: /nonexistent/python script.py") {
		t.Fatalf("shell line: %q", l)
	}
	// No Python named (none with Playwright yet): the browser, and the
	// plain Python as without one.
	f.w.python = pythonInfo{version: "3.11.2", at: time.Now()}
	f.writeBrowserJSON(`{"port": 41235, "app": "Chromium", "kind": "host"}`)
	help = f.freshHelp()
	if l := helpLine(help, "Shell: "); !strings.HasSuffix(l, "; Python 3.11.2 (no Playwright)") {
		t.Fatalf("shell line: %q", l)
	}
	if !strings.Contains(help, "Browser: a Chromium window on this computer (Chromium), CDP at http://127.0.0.1:41235 ") {
		t.Fatal(help)
	}
}

// A browser.json whose port doesn't answer is still shown: the app removes
// it when the browser ends, so it is stale only after the app was killed,
// and checking the port would slow every help (see browserInfo).
func TestWorkerHelpShowsAStaleBrowserJSON(t *testing.T) {
	f := newWorkerFixture(t)
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	port := l.Addr().(*net.TCPAddr).Port
	l.Close() // nothing answers on it now
	f.writeBrowserJSON(fmt.Sprintf(`{"host": "127.0.0.1", "port": %d, "app": "Edge", "kind": "host"}`, port))
	if help := f.freshHelp(); !strings.Contains(help, fmt.Sprintf("CDP at http://127.0.0.1:%d ", port)) {
		t.Fatal(help)
	}
}

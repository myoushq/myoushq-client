package myous

// The browser announcement. myous (the desktop app) starts the worker's
// browser and writes browser.json (see browserInfo); an agent that paired
// before that, and has no reason to send help again, would not know the
// browser is there. So the worker tells its approved contacts once per
// browser, with the help's own Browser and Python lines.

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"
)

// browserToldFile, in the worker's home, is the browser last announced:
// {"port": N, "python": "…"}. A worker restarted with the same browser
// stays silent.
const browserToldFile = "browser-told.json"

// browserKey is what makes a browser new to the worker's contacts.
type browserKey struct {
	Port   int    `json:"port"`
	Python string `json:"python"`
}

// browserAnnounce is what announceBrowser keeps between ticks.
type browserAnnounce struct {
	mu      sync.Mutex
	mod     time.Time // browser.json's mtime and size at the last look
	size    int64
	settled bool                // that look is done with: nothing to do until the file changes
	key     browserKey          // the browser being announced
	told    map[string]struct{} // contacts (npub) told about key, while some aren't yet
	failed  map[string]struct{} // contacts whose failure was logged, once per browser
}

// announceBrowser runs every tick: a stat of browser.json, and when it
// names a browser other than the one last announced (browser-told.json),
// a message to every approved contact. browser-told.json is written once
// all were told; a contact that couldn't be reached is tried again on the
// next tick, and the ones already told aren't told again. A browser.json
// that goes away says nothing (the help no longer lists a browser), and
// there's no one to tell without contacts: contacts paired later get the
// browser from the help.
func (w *Worker) announceBrowser(ctx context.Context) {
	if w.home == "" {
		return
	}
	a := &w.announce
	a.mu.Lock()
	defer a.mu.Unlock()
	info, err := os.Stat(filepath.Join(w.home, "browser.json"))
	if err != nil {
		a.settled = false
		return
	}
	if a.settled && info.ModTime().Equal(a.mod) && info.Size() == a.size {
		return
	}
	a.mod, a.size, a.settled = info.ModTime(), info.Size(), false
	b, ok := w.browser()
	if !ok { // not this worker's browser, or half written: looked at again when it changes
		a.settled = true
		return
	}
	key := browserKey{Port: b.port, Python: b.python}
	if told, ok := w.browserTold(); ok && told == key {
		a.settled = true
		return
	}
	if key != a.key || a.told == nil {
		a.key, a.told, a.failed = key, map[string]struct{}{}, map[string]struct{}{}
	}
	contacts, err := w.agent.Contacts()
	if err != nil {
		w.say(fmt.Sprintf("could not read the contacts to announce the browser: %v", err))
		return
	}
	var due []Contact
	for _, c := range contacts {
		if _, told := a.told[c.Npub]; c.Status == Approved && !told {
			due = append(due, c)
		}
	}
	sort.Slice(due, func(i, j int) bool { return due[i].Npub < due[j].Npub })
	text := w.browserAnnouncement(b)
	done := true
	for _, c := range due {
		if ctx.Err() != nil {
			return
		}
		if _, err := w.agent.Send(ctx, c.Npub, text); err != nil {
			done = false
			if _, logged := a.failed[c.Npub]; !logged {
				a.failed[c.Npub] = struct{}{}
				w.say(fmt.Sprintf("could not tell %s the browser is ready (trying again every %d s): %v",
					c.Alias, int(workerTick/time.Second), err))
			}
			continue
		}
		a.told[c.Npub] = struct{}{}
		w.say(fmt.Sprintf("told %s the browser is ready (CDP at http://%s:%d)", c.Alias, b.host, b.port))
	}
	if !done {
		return
	}
	if err := w.writeBrowserTold(key); err != nil {
		w.say(fmt.Sprintf("could not write %s: %v", browserToldFile, err))
		return // tried again next tick; the contacts told aren't told again
	}
	a.settled = true
}

// browserAnnouncement is the message: the help's Browser and Python lines.
func (w *Worker) browserAnnouncement(b browserInfo) string {
	lines := []string{"The worker's browser is ready.", browserLine(b)}
	if py := w.browserPythonLine(b); py != "" {
		lines = append(lines, py)
	}
	lines = append(lines, "Send help for the rest.")
	return strings.Join(lines, "\n")
}

func (w *Worker) browserTold() (browserKey, bool) {
	raw, err := os.ReadFile(filepath.Join(w.home, browserToldFile))
	if err != nil {
		return browserKey{}, false
	}
	var k browserKey
	if json.Unmarshal(raw, &k) != nil {
		return browserKey{}, false
	}
	return k, true
}

func (w *Worker) writeBrowserTold(k browserKey) error {
	raw, err := json.Marshal(k)
	if err != nil {
		return err
	}
	path := filepath.Join(w.home, browserToldFile)
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, raw, 0o666); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}

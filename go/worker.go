package myous

// A worker: an agent that executes requests from its approved contacts
// (protocol.md, section 7). Typically a process or container on its
// owner's machine, paired with the owner's other agents, which push
// files, pull files and run commands on it with `myous cp` and `myous
// exec`.
//
// The worker does what it's asked, one request at a time, after its review
// allows it (review.go); a request it only gets to after its ttl (the
// requester stopped waiting) is answered "expired" and never runs. It is a
// port of the Python worker
// (python/myous/worker.py) and behaves the same: the same replies, and the
// same files for the owner's app (worker.json, worker.log,
// requests/<id>.json, commands/, approvals/), so either can serve the same
// data directory.

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"os"
	"os/exec"
	"os/user"
	"path/filepath"
	"runtime"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/nbd-wtf/go-nostr"
)

// WorkerAbout is what a worker's profile says it is.
const WorkerAbout = "myoushq worker"

const (
	workerMaxTimeout     = 600
	workerDefaultTimeout = 120
	helpInterval         = 60 * time.Second  // between help replies to the same contact
	reviewTimeout        = 150 * time.Second // the hook may wait for the owner (120 s), inside an agent's default wait
	statusEvery          = 5 * time.Second
	workerTick           = 5 * time.Second // status writes and the owner's commands
	requestsKeep         = 200             // request records kept in <home>/requests for the owner's app
	stopFileMaxAge       = 600 * time.Second
	reconnectWait        = 10 * time.Second

	windowsHelp = "This worker runs on Windows: commands are PowerShell, not Linux shell commands " +
		"(use `Get-ChildItem` or `dir`, not `ls`; `Select-String`, not `grep`)."
	windowsHint     = "This worker runs PowerShell on Windows; send `help` for what it takes."
	notRecognized   = "is not recognized as the name of a cmdlet"
	invitePairingIn = 30 // seconds: an invite closer than this to expiring is renewed

	// Request ids name files the owner's app reads and writes
	// (requests/<id>.json, approvals/<id>.json and .answer,
	// commands/stop-<id>), so they must be plain names (validID).
	badID = "bad request: the id must be 1 to 64 characters from A-Z, a-z, 0-9, - and _"
	// A request is good for its ttl from when it was sent (protocol.md
	// 7.1): it must not run after its requester stopped waiting for it.
	requestTTL    = 600   // seconds, for a request without a ttl; also how long a help or other text gets a reply
	requestMaxTTL = 86400 // a longer ttl counts as this: the relay keeps messages a day
	badTTL        = "bad request: the ttl must be a number of seconds, at least 1"
	// Stopping the worker (Run's context ending: SIGTERM, SIGINT) ends the
	// command it is running and answers that request, then Run returns.
	stoppingWhy       = "stopped by the owner (the worker is shutting down)"
	shutdownReplyWait = 10 * time.Second // how long an answer may take once the worker is stopping
	// The owner's app can also stop the worker with commands/quit: the
	// same as SIGTERM, and on Windows (no signals for a worker without a
	// console) the only way short of killing it.
	quitCommand = "quit"
)

// validID is whether a request id is a plain name: 1 to 64 characters
// from A-Z, a-z, 0-9, - and _ (clients send 32 hex). The Python worker,
// review.py and the owner's apps hold ids to the same rule.
func validID(id string) bool {
	if len(id) == 0 || len(id) > 64 {
		return false
	}
	for i := 0; i < len(id); i++ {
		c := id[i]
		if !('a' <= c && c <= 'z' || 'A' <= c && c <= 'Z' || '0' <= c && c <= '9' || c == '-' || c == '_') {
			return false
		}
	}
	return true
}

// answerContext is ctx for answering a request: it ends shutdownReplyWait
// after ctx does, so a request cut short by the worker stopping still gets
// its answer.
func answerContext(ctx context.Context) (context.Context, context.CancelFunc) {
	actx, cancel := context.WithCancel(context.WithoutCancel(ctx))
	stop := context.AfterFunc(ctx, func() { time.AfterFunc(shutdownReplyWait, cancel) })
	return actx, func() { stop(); cancel() }
}

// WorkerOptions are a worker's settings (`myous worker`'s flags).
type WorkerOptions struct {
	Work          string   // the work directory, created if missing
	Review        string   // a review command; "" applies worker/review.py's rules built in
	PauseFile     string   // refuse requests while it exists (default <home>/worker.paused)
	AllowAbsolute bool     // allow absolute paths in cp requests
	Notes         []string // lines added to the help text (its browser is read from <home>/browser.json)
	Out           io.Writer
}

// workerAgent is what the worker needs from its agent (an *Agent; tests
// use a fake).
type workerAgent interface {
	Alias() string
	Card() string
	Npub() (string, error)
	Contacts() (map[string]Contact, error)
	Invite(ctx context.Context, cc ...ContactContext) (*Invite, error)
	Send(ctx context.Context, name, text string) (Entry, error)
	SyncCards(ctx context.Context) ([]Contact, error)
	SendFile(ctx context.Context, name, path string, extraTags nostr.Tags) (Entry, error)
	FetchBytes(ctx context.Context, e Entry) ([]byte, error)
	Listen(ctx context.Context, onNew func([]Entry), onTick func(), tick time.Duration) error
}

// Worker serves requests until stopped; see NewWorker and Run.
type Worker struct {
	agent         workerAgent
	st            Storage
	home          string // "" for storage that isn't a directory: no log, records or commands
	work          string
	reviewCmd     string
	review        *builtinReview // when there is no review command
	pauseFile     string
	logFile       string
	commandsDir   string
	quitFile      string // commands/quit: stop, as on SIGTERM
	requestsDir   string
	allowAbsolute bool
	notes         []string
	out           io.Writer
	windows       bool // PowerShell, and the help and hints that say so
	started       int64
	startedAt     time.Time

	// The operations, replaceable in tests.
	execFn func(context.Context, Entry, map[string]any, string) error
	getFn  func(context.Context, Entry, map[string]any, string) error

	mu            sync.Mutex // what follows, and the owner's side (status, invite, commands)
	requests      int
	last          pyObj
	paired        pyObj // the most recent pairing, with its verification code
	invite        *Invite
	helpSent      map[string]time.Time
	statusWritten time.Time
	python        pythonInfo
	browserPy     browserPython

	announce browserAnnounce // the browser announcement (announceBrowser)

	busy  sync.Mutex // one request at a time, whichever path delivered it
	sayMu sync.Mutex
}

// NewWorker makes a worker for agent, with its data in st (a
// *FileStorage gives it the files the owner's app reads).
func NewWorker(agent *Agent, st Storage, opts WorkerOptions) (*Worker, error) {
	return newWorker(agent, st, opts)
}

func newWorker(agent workerAgent, st Storage, opts WorkerOptions) (*Worker, error) {
	now := time.Now()
	w := &Worker{agent: agent, st: st, allowAbsolute: opts.AllowAbsolute, notes: opts.Notes, out: opts.Out,
		windows: isWindows, started: now.Unix(), startedAt: now, helpSent: map[string]time.Time{}}
	if w.out == nil {
		w.out = os.Stdout
	}
	w.execFn, w.getFn = w.doExec, w.doGet
	if fs, ok := st.(*FileStorage); ok {
		w.home = fs.Home
	}
	work, err := filepath.Abs(expandUser(opts.Work))
	if err != nil {
		return nil, err
	}
	w.work = realPath(work)
	if err := os.MkdirAll(w.work, 0o777); err != nil {
		return nil, err
	}
	if w.reviewCmd, err = checkReviewCmd(opts.Review, w.work, w.windows); err != nil {
		return nil, err
	}
	w.pauseFile = opts.PauseFile
	if w.pauseFile == "" && w.home != "" {
		w.pauseFile = filepath.Join(w.home, "worker.paused")
	}
	if w.home != "" {
		w.logFile = filepath.Join(w.home, "worker.log")
		// The owner's side (the desktop app): commands it drops as files,
		// and one record per request it can show, with the output.
		w.commandsDir = filepath.Join(w.home, "commands")
		w.quitFile = filepath.Join(w.commandsDir, quitCommand)
		w.requestsDir = filepath.Join(w.home, "requests")
	}
	if w.reviewCmd == "" {
		// review.py, built in. Its pause file is $MYOUS_PAUSE_FILE or
		// <home>/worker.paused; an explicit --pause-file wins.
		pause := opts.PauseFile
		if pause == "" {
			pause = os.Getenv("MYOUS_PAUSE_FILE")
		}
		if pause == "" {
			pause = w.pauseFile
		}
		w.review = &builtinReview{home: w.home, pauseFile: pause, wait: approvalWait(), limit: reviewTimeout, windows: w.windows}
	}
	return w, nil
}

// checkReviewCmd is the review command with its relative paths made
// absolute now, while the current directory is still the owner's. It is
// the one control point, so it must not be something a request could
// replace: nothing in it may live under the work directory.
func checkReviewCmd(cmd, work string, windows bool) (string, error) {
	if cmd == "" {
		return "", nil
	}
	// Windows paths have backslashes, which POSIX quoting would eat; there
	// the words keep their quotes and the command runs under cmd.exe.
	words, err := shlexSplit(cmd, !windows)
	if err != nil {
		return "", err
	}
	for i, word := range words {
		bare := word
		if windows {
			bare = strings.Trim(word, `"`)
		}
		p := expandUser(bare)
		if p == "" {
			p = "." // as Python's Path("")
		}
		if !exists(p) {
			continue
		}
		if abs, err := filepath.Abs(p); err == nil {
			p = realPath(abs)
		}
		if windows {
			words[i] = `"` + p + `"`
		} else {
			words[i] = p
		}
		if within(p, work, windows) {
			return "", fmt.Errorf("the review hook (%s) is inside the work directory %s, "+
				"where requests can replace it; keep it elsewhere", bare, work)
		}
	}
	if windows {
		return strings.Join(words, " "), nil
	}
	for i, word := range words {
		words[i] = shlexQuote(word)
	}
	return strings.Join(words, " "), nil
}

// --- running -------------------------------------------------------------

// Run serves requests until ctx ends, or until the owner's app creates
// commands/quit, which does the same. It reconnects if the relay drops; it
// returns an error only for what reconnecting can't fix.
func (w *Worker) Run(ctx context.Context) error {
	ctx, quit := context.WithCancel(ctx)
	defer quit()
	if w.quitFile != "" {
		w.dropStaleQuit()
		defer os.Remove(w.quitFile) // left by an app that also sent SIGTERM
		go w.watchQuit(ctx, quit)
	}
	w.mu.Lock()
	err := w.ensureInvite(ctx)
	if err == nil {
		err = w.writeStatus()
	}
	w.mu.Unlock()
	if err != nil {
		return err
	}

	// Requests are handled one at a time, in order, away from the
	// connection, which keeps receiving (and the tick keeps writing the
	// status) while a command runs.
	q := &entryQueue{wake: make(chan struct{}, 1)}
	var handling sync.WaitGroup
	handling.Add(1)
	go func() {
		defer handling.Done()
		for {
			for batch := q.pop(); batch != nil; batch = q.pop() {
				if ctx.Err() != nil {
					return
				}
				w.onNew(ctx, batch)
			}
			select {
			case <-ctx.Done():
				return
			case <-q.wake:
			}
		}
	}()
	defer handling.Wait()

	for {
		lctx, cancel := context.WithCancel(ctx)
		var tickMu sync.Mutex
		var tickErr error
		err := w.agent.Listen(lctx, q.push, func() {
			if err := w.tick(lctx); err != nil && lctx.Err() == nil {
				tickMu.Lock()
				tickErr = err
				tickMu.Unlock()
				cancel()
			}
		}, workerTick)
		cancel()
		if ctx.Err() != nil {
			w.say("stopping")
			return nil
		}
		tickMu.Lock()
		if tickErr != nil {
			err = tickErr
		}
		tickMu.Unlock()
		if !retryable(err) {
			return err
		}
		w.say(fmt.Sprintf("connection lost (%v); retrying in 10 seconds", err))
		select {
		case <-ctx.Done():
			return nil
		case <-time.After(reconnectWait):
		}
	}
}

// retryable is whether an error is the network's (try again), rather than
// the hub refusing or the identity being wrong (as the Python worker,
// which retries OSError and RelayError only).
func retryable(err error) bool {
	var he *HubError
	var ie *IdentityError
	var pe *PairingError
	return !errors.As(err, &he) && !errors.As(err, &ie) && !errors.As(err, &pe)
}

// entryQueue hands Listen's entries to the request goroutine, in order.
type entryQueue struct {
	mu    sync.Mutex
	items [][]Entry
	wake  chan struct{}
}

func (q *entryQueue) push(entries []Entry) {
	q.mu.Lock()
	q.items = append(q.items, entries)
	q.mu.Unlock()
	select {
	case q.wake <- struct{}{}:
	default:
	}
}

func (q *entryQueue) pop() []Entry {
	q.mu.Lock()
	defer q.mu.Unlock()
	if len(q.items) == 0 {
		return nil
	}
	batch := q.items[0]
	q.items = q.items[1:]
	return batch
}

// tick: the owner's commands, a fresh invite while unpaired, the status,
// then the browser announcement (outside w.mu: it looks up the Pythons).
func (w *Worker) tick(ctx context.Context) error {
	if err := func() error {
		w.mu.Lock()
		defer w.mu.Unlock()
		if err := w.handleCommands(ctx); err != nil {
			return err
		}
		if err := w.ensureInvite(ctx); err != nil {
			return err
		}
		if time.Since(w.statusWritten) > statusEvery {
			return w.writeStatus()
		}
		return nil
	}(); err != nil {
		return err
	}
	w.announceBrowser(ctx)
	return nil
}

// --- the owner's commands ------------------------------------------------

// watchQuit ends Run (quit) once commands/quit appears: the running
// command is ended and its request answered, as on SIGTERM. Checked as
// often as a command's stop file, and apart from the connection's tick,
// so it works while reconnecting too.
func (w *Worker) watchQuit(ctx context.Context, quit context.CancelFunc) {
	t := time.NewTicker(stopEvery)
	defer t.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-t.C:
		}
		if exists(w.quitFile) {
			os.Remove(w.quitFile)
			w.say("stopping: the owner's app asked (commands/quit)")
			quit()
			return
		}
	}
}

// dropStaleQuit removes a commands/quit from before this worker started
// (its worker was killed before it could take it), so it doesn't stop
// this one. Whole seconds: some file systems keep no finer times.
func (w *Worker) dropStaleQuit() {
	if info, err := os.Stat(w.quitFile); err == nil && info.ModTime().Before(w.startedAt.Truncate(time.Second)) {
		os.Remove(w.quitFile)
	}
}

// handleCommands runs the commands from the owner's app: a file per
// command in <home>/commands, removed once done. `new-code` replaces the
// open pairing code (while unpaired); `unpair` drops every contact and
// offers a new code; `quit` is watchQuit's. Called with w.mu held.
func (w *Worker) handleCommands(ctx context.Context) error {
	if w.commandsDir == "" {
		return nil
	}
	files, err := os.ReadDir(w.commandsDir)
	if err != nil {
		return nil // not there (yet)
	}
	for _, f := range files { // sorted by name
		path := filepath.Join(w.commandsDir, f.Name())
		if f.Name() == quitCommand {
			continue
		}
		if strings.HasPrefix(f.Name(), "stop-") {
			// For the command running now (runCommand watches and removes
			// it); one left behind by a command that had finished expires.
			if info, err := os.Stat(path); err == nil && time.Since(info.ModTime()) > stopFileMaxAge {
				os.Remove(path)
			}
			continue
		}
		if f.IsDir() || os.Remove(path) != nil {
			continue
		}
		switch f.Name() {
		case "new-code":
			if contacts, err := w.agent.Contacts(); err == nil && len(contacts) == 0 {
				w.invite = nil
				if err := w.ensureInvite(ctx); err != nil {
					return err
				}
			}
		case "unpair":
			if err := w.unpair(ctx); err != nil {
				return err
			}
		default:
			w.say("unknown command file " + f.Name())
		}
	}
	return nil
}

// unpair drops every contact and offers a new code. Called with w.mu held.
func (w *Worker) unpair(ctx context.Context) error {
	unlock, _, err := lock(w.st, "state", true)
	if err != nil {
		return err
	}
	err = w.st.Put("contacts", map[string]Contact{})
	unlock()
	if err != nil {
		return err
	}
	w.paired = nil
	w.invite = nil
	w.log(pyObj{{"op", "unpair"}})
	w.say("unpaired: contacts removed; a new pairing code follows")
	if err := w.ensureInvite(ctx); err != nil {
		return err
	}
	return w.writeStatus()
}

// ensureInvite keeps a fresh invite on offer until the worker has a
// contact, so the owner can pair their agent with it. Called with w.mu
// held.
func (w *Worker) ensureInvite(ctx context.Context) error {
	contacts, err := w.agent.Contacts()
	if err != nil {
		return err
	}
	if len(contacts) > 0 {
		w.invite = nil
		return nil
	}
	if w.invite != nil && w.invite.ExpiresAt > time.Now().Unix()+invitePairingIn {
		return nil
	}
	inv, err := w.agent.Invite(ctx)
	if err != nil {
		return err
	}
	w.invite = inv
	w.say(fmt.Sprintf("pairing code %s (link %s; valid 15 minutes, renewed when it expires). Tell your agent: %s",
		inv.Code, inv.Link, pairingMessage(inv.Code, w.agent.Alias(), w.agent.Card(), w.windows)))
	return w.writeStatus()
}

// onNew handles new entries: pairing results, cards, and requests, one at
// a time.
func (w *Worker) onNew(ctx context.Context, entries []Entry) {
	w.busy.Lock()
	defer w.busy.Unlock()
	for _, e := range entries {
		if e.Direction == "out" {
			continue
		}
		if ctx.Err() != nil {
			return
		}
		switch e.Type {
		case "paired":
			// Owners compare this number on both sides, so it must be visible here too.
			w.mu.Lock()
			w.paired = pyObj{{"alias", e.Alias}, {"verify", e.Verify}, {"at", time.Now().Unix()}}
			w.log(pyObj{{"op", "paired"}, {"alias", e.Alias}, {"sender", e.Peer}, {"verify", e.Verify}})
			w.say(fmt.Sprintf("paired with %s: verification code %s (your agent shows the same number; compare them)", e.Alias, e.Verify))
			if err := w.ensureInvite(ctx); err != nil {
				w.say(fmt.Sprintf("could not renew the invite: %v", err))
			}
			w.mu.Unlock()
			w.agent.SyncCards(ctx) // tell it what this worker is, now; the next round does it if this fails
		case "card":
			w.log(pyObj{{"op", "card"}, {"alias", e.Alias}, {"sender", e.Peer}})
			w.say(e.Text)
		case "message", "file":
			w.handle(ctx, e)
		}
		w.mu.Lock()
		if err := w.writeStatus(); err != nil {
			w.say(fmt.Sprintf("could not write the status: %v", err))
		}
		w.mu.Unlock()
	}
}

// badRequest is a failure caused by the request itself: it gets a
// refusal ("bad request: ...").
type badRequest struct{ err error }

func (b badRequest) Error() string { return b.err.Error() }

// handle serves one request, in arrival order. Whatever is wrong with a
// request gets a refusal, never a dead worker.
func (w *Worker) handle(ctx context.Context, e Entry) {
	rid := ""
	failed := func(err error, bad bool) {
		if ctx.Err() != nil && rid != "" && validID(rid) {
			// Cut short by the worker stopping: the requester is told so.
			w.log(pyObj{{"op", "error"}, {"id", rid}, {"sender", e.Peer}, {"error", err.Error()}})
			if err2 := w.refuse(ctx, e, rid, stoppingWhy); err2 != nil {
				w.say(fmt.Sprintf("could not answer request %s: %v", rid, err2))
			}
			return
		}
		if !bad {
			// Our side (relay, hub, disk): log it; the requester's command times out.
			w.log(pyObj{{"op", "error"}, {"id", nullable(rid)}, {"sender", e.Peer}, {"error", err.Error()}})
			who := rid
			if who == "" {
				who = "?"
			}
			w.say(fmt.Sprintf("request %s failed: %v", who, err))
			return
		}
		w.log(pyObj{{"op", "error"}, {"id", nullable(rid)}, {"sender", e.Peer}, {"error", err.Error()}})
		if rid != "" {
			if err2 := w.refuse(ctx, e, rid, fmt.Sprintf("bad request: %v", err)); err2 != nil {
				w.say(fmt.Sprintf("could not answer request %s: %v", rid, err))
			}
		}
	}
	defer func() {
		if r := recover(); r != nil { // a malformed request must not kill the worker
			failed(fmt.Errorf("%v", r), true)
		}
	}()
	err := func() error {
		if err := os.MkdirAll(w.work, 0o777); err != nil { // a command may have removed it
			return err
		}
		if e.Type == "file" {
			if len(e.W) >= 3 && e.W[0] == "put" {
				rid = e.W[1]
				if !validID(rid) {
					return w.refuse(ctx, e, rid, badID)
				}
				// Before the download: an expired put isn't fetched (its size is the file message's).
				ttl, ok := tagTTL(e.W)
				if !ok {
					return w.refuse(ctx, e, rid, badTTL)
				}
				if expired, err := w.expire(ctx, e, rid, ttl, "put", pyKV{"path", e.W[2]}, pyKV{"size", e.Size}); expired {
					return err
				}
				return w.doPut(ctx, e, e.W[1], e.W[2])
			}
			return nil
		}
		obj, isRequest := parseRequest(e.Text)
		var req map[string]any
		if isRequest && (obj["myous"] == "exec" || obj["myous"] == "get") {
			req = obj
			rid = obj["id"].(string)
			if !validID(rid) {
				return w.refuse(ctx, e, rid, badID)
			}
			v, present := req["ttl"]
			ttl, ok := jsonTTL(v, present)
			if !ok {
				return w.refuse(ctx, e, rid, badTTL)
			}
			op, key := req["myous"].(string), "cmd"
			if op == "get" {
				key = "path"
			}
			if expired, err := w.expire(ctx, e, rid, ttl, op, pyKV{key, req[key]}); expired {
				return err
			}
		}
		switch {
		case req != nil && req["myous"] == "exec":
			return w.execFn(ctx, e, req, rid)
		case req != nil && req["myous"] == "get":
			return w.getFn(ctx, e, req, rid)
		case !isRequest:
			return w.sendHelp(ctx, e)
		}
		return nil
	}()
	if err != nil {
		var bad badRequest
		failed(err, errors.As(err, &bad))
	}
}

// jsonTTL is the ttl of an exec or get (its "ttl"; none, or null, is
// requestTTL) in whole seconds, at most requestMaxTTL; ok is false when it
// isn't a number of seconds of at least 1 (a bool, a string, NaN,
// ±Infinity). An integer too long for a float is still a number: above
// the maximum.
func jsonTTL(v any, present bool) (int64, bool) {
	if !present || v == nil {
		return requestTTL, true
	}
	f, ok := pyNumber(v)
	if !ok || math.IsNaN(f) || (math.IsInf(f, 0) && strings.ContainsAny(v.(json.Number).String(), ".eE")) {
		return 0, false
	}
	return clampTTL(f)
}

// tagTTL is the ttl of a put, the w tag's fifth value: a decimal string,
// digits only; without one, requestTTL.
func tagTTL(w []string) (int64, bool) {
	if len(w) < 4 {
		return requestTTL, true
	}
	if !asciiDigits(w[3]) {
		return 0, false
	}
	f, err := strconv.ParseFloat(w[3], 64)
	if err != nil && !math.IsInf(f, 0) {
		return 0, false
	}
	return clampTTL(f)
}

func clampTTL(f float64) (int64, bool) {
	if !(f >= 1) {
		return 0, false
	}
	return int64(math.Floor(min(f, requestMaxTTL))), true
}

func asciiDigits(s string) bool {
	for i := 0; i < len(s); i++ {
		if s[i] < '0' || s[i] > '9' {
			return false
		}
	}
	return s != ""
}

// requestAge is how long ago an entry was sent, in whole seconds by the
// worker's clock: 0 when the sender's clock is ahead.
func requestAge(sentAt int64, now time.Time) int64 {
	return max(now.Unix()-sentAt, 0)
}

// expiredWhy is the refusal of a request sent age seconds ago whose ttl
// had passed (protocol.md 7.1); the Python worker says the same.
func expiredWhy(age, ttl int64) string {
	ago := fmt.Sprintf("%d seconds", age)
	switch {
	case age >= 7200:
		ago = fmt.Sprintf("%d hours", age/3600)
	case age >= 120:
		ago = fmt.Sprintf("%d minutes", age/60)
	}
	return fmt.Sprintf("expired: sent %s ago, its ttl is %d seconds; not run", ago, ttl)
}

// expire answers a request that was sent longer ago than its ttl, before
// it reaches the review hook or runs: its requester has stopped waiting
// (the worker was off, or busy with earlier requests), so it must not run
// now. expired is false for a request still good (or one without a send
// time); err is the answer's. fields are the request's, as the review
// would get them. It is recorded as "expired", not as a refusal: the
// owner's app notifies each recent refusal, and a worker that comes back
// to a backlog would flood its owner.
func (w *Worker) expire(ctx context.Context, e Entry, rid string, ttl int64, op string, fields ...pyKV) (expired bool, err error) {
	if e.SentAt == 0 {
		return false, nil
	}
	now := time.Now()
	age := requestAge(e.SentAt, now)
	if age <= ttl {
		return false, nil
	}
	why := expiredWhy(age, ttl)
	request := append(pyObj{{"op", op}, {"id", rid}, {"sender", e.Peer}, {"alias", e.Alias}}, fields...)
	w.log(request.with("decision", "expired").with("reason", why))
	alias := e.Alias
	if alias == "" {
		alias = "?"
	}
	w.say(fmt.Sprintf("%s request %s from %s %s", op, rid, alias, why))
	w.note(e, "expired", false)
	w.record(rid, append(request, pyKV{"at", now.Unix()}, pyKV{"done_at", now.Unix()}, pyKV{"decision", "expired"},
		pyKV{"reason", why}))
	return true, w.reply(ctx, e, pyObj{{"myous", "ack"}, {"id", rid}, {"ok", false}, {"error", why}})
}

// parseRequest is the JSON object of a worker request or reply
// (protocol.md 7.1): an object with string "myous" and "id".
func parseRequest(text string) (map[string]any, bool) {
	if !strings.HasPrefix(text, "{") {
		return nil, false
	}
	v, err := pyLoads(text)
	if err != nil {
		return nil, false
	}
	obj, ok := v.(map[string]any)
	if !ok {
		return nil, false
	}
	_, okOp := obj["myous"].(string)
	_, okID := obj["id"].(string)
	return obj, okOp && okID
}

// --- the three operations ------------------------------------------------

func (w *Worker) doExec(ctx context.Context, e Entry, req map[string]any, rid string) error {
	cmd, ok := req["cmd"].(string)
	if !ok || pyStrip(cmd) == "" {
		return w.refuse(ctx, e, rid, "exec needs a cmd string")
	}
	timeout := float64(workerDefaultTimeout)
	if v, present := req["timeout"]; present {
		if f, ok := pyNumber(v); ok && !math.IsInf(f, 0) && !math.IsNaN(f) {
			timeout = math.Trunc(f)
		}
	}
	seconds := int(min(max(timeout, 1), workerMaxTimeout))
	if why := w.reviewRequest(ctx, e, rid, pyObj{{"op", "exec"}}, pyKV{"cmd", cmd}); why != "" {
		return w.refuse(ctx, e, rid, why)
	}
	started := time.Now()
	// The id is a plain name (handle checked it): "stop-../../key" can't
	// reach outside commands/.
	stopFile := ""
	if w.commandsDir != "" && validID(rid) {
		stopFile = filepath.Join(w.commandsDir, "stop-"+rid)
	}
	r, err := runCommand(ctx, cmd, w.work, seconds, stopFile)
	if err != nil {
		return err
	}
	if w.windows && strings.Contains(r.Stderr, notRecognized) {
		// An agent that took this for a Linux worker: say so where it looks.
		if r.Stderr != "" && !strings.HasSuffix(r.Stderr, "\n") {
			r.Stderr += "\n"
		}
		r.Stderr += windowsHint
	}
	w.note(e, "exec", true)
	result := pyObj{{"exit", r.Exit}, {"stdout", r.Stdout}, {"stderr", r.Stderr}, {"truncated", r.Truncated}}
	if r.Stopped {
		result = append(result, pyKV{"stopped", true})
	}
	duration := math.Round(time.Since(started).Seconds()*100) / 100
	w.record(rid, append(pyObj{{"done_at", time.Now().Unix()}, {"duration", duration}}, result...))
	return w.reply(ctx, e, append(result, pyKV{"myous", "result"}, pyKV{"id", rid}))
}

func (w *Worker) doGet(ctx context.Context, e Entry, req map[string]any, rid string) error {
	path := req["path"]
	target, why := w.resolve(path)
	if why == "" {
		why = w.reviewRequest(ctx, e, rid, pyObj{{"op", "get"}}, pyKV{"path", path})
	}
	var info os.FileInfo
	if why == "" {
		var err error
		if info, err = os.Stat(target); err != nil || !info.Mode().IsRegular() {
			why = "no such file on the worker"
		}
	}
	if why != "" {
		return w.refuse(ctx, e, rid, why)
	}
	if _, err := w.agent.SendFile(ctx, e.Peer, target, nostr.Tags{{"w", "file", rid}}); err != nil {
		var re *RelayError
		if errors.As(err, &re) || ctx.Err() != nil {
			return err // handle answers "stopped" when the worker is stopping
		}
		return w.refuse(ctx, e, rid, fmt.Sprintf("could not send the file: %v", err))
	}
	w.note(e, "get", true)
	w.record(rid, pyObj{{"done_at", time.Now().Unix()}, {"size", info.Size()}})
	return nil
}

func (w *Worker) doPut(ctx context.Context, e Entry, rid, path string) error {
	target, why := w.resolve(path)
	if why == "" {
		why = w.reviewRequest(ctx, e, rid, pyObj{{"op", "put"}}, pyKV{"path", path}, pyKV{"size", e.Size})
	}
	if why != "" {
		return w.refuse(ctx, e, rid, why)
	}
	data, err := w.agent.FetchBytes(ctx, e)
	if err == nil {
		if err = os.MkdirAll(filepath.Dir(target), 0o777); err == nil {
			err = os.WriteFile(target, data, 0o666)
		}
	}
	if err != nil {
		if ctx.Err() != nil {
			return w.refuse(ctx, e, rid, stoppingWhy)
		}
		return w.refuse(ctx, e, rid, fmt.Sprintf("could not write the file: %v", err))
	}
	w.note(e, "put", true)
	w.record(rid, pyObj{{"done_at", time.Now().Unix()}, {"size", len(data)}})
	return w.reply(ctx, e, pyObj{{"myous", "ack"}, {"id", rid}, {"ok", true}, {"path", target}, {"size", len(data)},
		{"sha256", sha256Hex(data)}})
}

// --- review, paths, replies ----------------------------------------------

// reviewRequest is why the request is refused, or "" to go ahead
// (protocol.md 7.2): the review command, or review.py's rules built in.
func (w *Worker) reviewRequest(ctx context.Context, e Entry, rid string, op pyObj, fields ...pyKV) string {
	request := append(op, pyKV{"id", rid}, pyKV{"sender", e.Peer}, pyKV{"alias", e.Alias})
	request = append(request, fields...)
	// Recorded first: the review may ask the owner, and the app shows the
	// question at once.
	w.record(rid, append(append(pyObj(nil), request...), pyKV{"at", time.Now().Unix()}, pyKV{"decision", "pending"}))
	var why string
	if w.reviewCmd != "" {
		// Not in the work directory: the hook must not pick up anything a request wrote there.
		why = runReviewHook(ctx, w.reviewCmd, w.home, request, reviewTimeout)
	} else {
		why = w.review.decide(ctx, request)
	}
	decision, logged := "allow", "allow"
	if why != "" {
		decision, logged = "refuse", "refuse: "+why
	}
	w.log(request.with("decision", logged))
	w.record(rid, append(append(pyObj(nil), request...), pyKV{"at", time.Now().Unix()}, pyKV{"decision", decision},
		pyKV{"reason", nullable(why)}))
	return why
}

func (w *Worker) paused() bool { return w.pauseFile != "" && exists(w.pauseFile) }

// resolve is where a request's path points, if it stays within the work
// directory (or is absolute and absolute paths are allowed); else why not.
func (w *Worker) resolve(path any) (string, string) {
	s, ok := path.(string)
	if !ok || pyStrip(s) == "" || strings.Contains(s, "\x00") {
		return "", "a path is needed (a plain path, no NUL bytes)"
	}
	if isAbsolute(s, w.windows) {
		if !w.allowAbsolute {
			return "", "absolute paths are not allowed on this worker; use a path relative to its work directory"
		}
		return purePath(s, w.windows), ""
	}
	target := realPath(joinUnder(w.work, s, w.windows))
	if !within(target, w.work, w.windows) {
		return "", "the path leaves the worker's work directory"
	}
	return target, ""
}

func (w *Worker) refuse(ctx context.Context, e Entry, rid, why string) error {
	w.note(e, "refuse", false)
	w.record(rid, pyObj{{"done_at", time.Now().Unix()}, {"decision", "refuse"}, {"reason", why}})
	return w.reply(ctx, e, pyObj{{"myous", "ack"}, {"id", rid}, {"ok", false}, {"error", why}})
}

// reply sends a JSON reply, written as the Python worker writes it.
func (w *Worker) reply(ctx context.Context, e Entry, obj pyObj) error {
	text := pyDumps(obj, false)
	if _, err := splitMessage(text); err != nil {
		return badRequest{err} // too long to send: the requester gets a refusal instead
	}
	ctx, cancel := answerContext(ctx) // answered even when the worker is stopping
	defer cancel()
	_, err := w.agent.Send(ctx, e.Peer, text)
	return err
}

func (w *Worker) sendHelp(ctx context.Context, e Entry) error {
	if e.SentAt != 0 && requestAge(e.SentAt, time.Now()) > requestTTL {
		return nil // from before the worker came back: whoever asked has moved on
	}
	w.mu.Lock()
	if last, ok := w.helpSent[e.Peer]; ok && time.Since(last) < helpInterval {
		w.mu.Unlock()
		return nil
	}
	w.helpSent[e.Peer] = time.Now()
	w.mu.Unlock()
	_, err := w.agent.Send(ctx, e.Peer, w.helpText())
	return err
}

// helpText is the worker's answer to anything that isn't a request: what
// it is, its shell and what's installed, its limits.
func (w *Worker) helpText() string {
	alias := w.agent.Alias()
	lines := []string{alias + " is a myoushq worker: it runs commands and moves files for its owner's agents."}
	if w.windows {
		lines = append(lines, windowsHelp)
	}
	if card := w.agent.Card(); card != "" {
		lines = append(lines, "Its owner describes it: "+card)
	}
	shell := fmt.Sprintf("Shell: %s on %s %s", shellName(w.windows), systemName(), machine())
	b, hasBrowser := w.browser()
	py := w.pythonInfo()
	if hasBrowser && b.python != "" {
		shell += "; " + w.browserPythonLine(b)
	} else if py.version != "" {
		shell += "; Python " + py.version
		if py.playwright {
			shell += "; Playwright installed"
		} else {
			shell += " (no Playwright)"
		}
	}
	allowed := "; absolute paths refused)"
	if w.allowAbsolute {
		allowed = "; absolute paths allowed)"
	}
	lines = append(lines,
		"Use `myous exec <me> -- CMD`, `myous cp FILE <me>:PATH` and `myous cp <me>:PATH FILE` "+
			"(protocol.md, section 7). Wait for each reply before the next request.",
		shell,
	)
	if hasBrowser {
		lines = append(lines, browserLine(b))
	}
	lines = append(lines,
		"Work directory: "+w.work+" (paths are relative to it"+allowed,
		fmt.Sprintf("Limits: commands time out after %d s by default (%d s max); output is cut to %d KB; files up to %d MB.",
			workerDefaultTimeout, workerMaxTimeout, stdoutLimit/1000, maxBlob>>20),
	)
	lines = append(lines, w.notes...)
	return strings.Join(lines, "\n")
}

// browserLine is the help's Browser line, and the browser announcement's.
func browserLine(b browserInfo) string {
	app := ""
	if b.app != "" {
		app = " (" + b.app + ")"
	}
	return fmt.Sprintf("Browser: a Chromium window on this computer%s, CDP at http://%s:%d "+
		"(Playwright: chromium.connect_over_cdp, then browser.contexts[0], which has the logins). "+
		"Its owner logs into sites there; ask them to rather than logging in yourself.", app, b.host, b.port)
}

// browserPythonLine is the Python that drives the browser, as the help's
// Shell line and the browser announcement name it; "" when browser.json
// names none. It is myous's own, not the one on PATH: name it, and say
// when the plain one can't do the same.
func (w *Worker) browserPythonLine(b browserInfo) string {
	if b.python == "" {
		return ""
	}
	line := "Python with Playwright: " + w.programCall(b.python) + " script.py"
	var about []string
	if v := w.pythonVersion(b.python); v != "" {
		about = append(about, v)
	}
	if py := w.pythonInfo(); py.version != "" && !py.playwright {
		about = append(about, "the plain python on this computer has no Playwright")
	}
	if len(about) > 0 {
		line += " (" + strings.Join(about, "; ") + ")"
	}
	return line
}

// pythonInfo is the Python on this computer, for the help text (the
// Python worker names its own): its version and whether Playwright is
// installed for it. Looked up at most every ten minutes.
type pythonInfo struct {
	version    string
	playwright bool
	at         time.Time
}

func (w *Worker) pythonInfo() pythonInfo {
	w.mu.Lock()
	defer w.mu.Unlock()
	if !w.python.at.IsZero() && time.Since(w.python.at) < 10*time.Minute {
		return w.python
	}
	w.python = findPython(w.windows)
	w.python.at = time.Now()
	return w.python
}

func findPython(windows bool) pythonInfo {
	tries := [][]string{{"python3"}, {"python"}}
	if windows {
		tries = [][]string{{"py", "-3"}, {"python"}, {"python3"}}
	}
	const probe = "import platform, importlib.util; print(platform.python_version(), importlib.util.find_spec('playwright') is not None)"
	for _, t := range tries {
		path, err := exec.LookPath(t[0])
		if err != nil {
			continue
		}
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		c := exec.CommandContext(ctx, path, append(t[1:], "-c", probe)...)
		hideWindow(c)
		out, err := c.Output()
		cancel()
		if f := strings.Fields(string(out)); err == nil && len(f) == 2 {
			return pythonInfo{version: f[0], playwright: f[1] == "True"}
		}
	}
	return pythonInfo{}
}

// browserInfo is the worker's browser on this computer, from
// browser.json in its home: myous (the desktop app) writes it once the
// browser's DevTools port answers, with "kind" "host", and removes it when
// the browser ends. It is read on every help (a small file), so a browser
// started while the worker runs is in the next help. myous for Mac's
// browser.json (no kind) is for the container's forward.py, not for this.
//
// A browser.json whose port no longer answers is shown anyway: the app
// removes the file when the browser ends, so a stale one only outlives an
// app that was killed, and the app replaces it at the browser's next
// start. Checking the port would put a network wait into every help for
// that rare case.
type browserInfo struct {
	host   string
	port   int
	app    string
	python string // the Python with Playwright, "" when not named
}

func (w *Worker) browser() (browserInfo, bool) {
	if w.home == "" {
		return browserInfo{}, false
	}
	raw, err := os.ReadFile(filepath.Join(w.home, "browser.json"))
	if err != nil {
		return browserInfo{}, false
	}
	var v struct {
		Kind   string `json:"kind"`
		Host   string `json:"host"`
		Port   int    `json:"port"`
		App    string `json:"app"`
		Python string `json:"python"`
	}
	if json.Unmarshal(raw, &v) != nil || v.Kind != "host" || v.Port <= 0 || v.Port > 65535 {
		return browserInfo{}, false
	}
	if v.Host == "" {
		v.Host = "127.0.0.1"
	}
	return browserInfo{host: v.Host, port: v.Port, app: v.App, python: v.Python}, true
}

// programCall is a program as this worker's shell runs it: `& "C:\…"` in
// PowerShell (single quotes when the path has what double quotes would
// expand), sh quoting elsewhere.
func (w *Worker) programCall(p string) string {
	if w.windows {
		if strings.ContainsAny(p, "$`\"") {
			return "& '" + strings.ReplaceAll(p, "'", "''") + "'"
		}
		return `& "` + p + `"`
	}
	return shQuote(p)
}

func shQuote(p string) string {
	plain := p != ""
	for _, c := range p {
		if !(c < 128 && (c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' || strings.ContainsRune("/._-+,:@", c))) {
			plain = false
			break
		}
	}
	if plain {
		return p
	}
	return "'" + strings.ReplaceAll(p, "'", `'\''`) + "'"
}

// pythonVersion is the version of the Python at path (the browser's),
// looked up at most every ten minutes; "" when it doesn't run.
func (w *Worker) pythonVersion(path string) string {
	w.mu.Lock()
	defer w.mu.Unlock()
	if w.browserPy.path == path && !w.browserPy.at.IsZero() && time.Since(w.browserPy.at) < 10*time.Minute {
		return w.browserPy.version
	}
	version := ""
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	c := exec.CommandContext(ctx, path, "-c", "import platform; print(platform.python_version())")
	hideWindow(c)
	out, err := c.Output()
	cancel()
	if f := strings.Fields(string(out)); err == nil && len(f) == 1 {
		version = f[0]
	}
	w.browserPy = browserPython{path: path, version: version, at: time.Now()}
	return version
}

type browserPython struct {
	path, version string
	at            time.Time
}

// systemName is Python's platform.system().
func systemName() string {
	switch runtime.GOOS {
	case "darwin":
		return "Darwin"
	case "linux":
		return "Linux"
	case "windows":
		return "Windows"
	case "freebsd":
		return "FreeBSD"
	case "openbsd":
		return "OpenBSD"
	case "netbsd":
		return "NetBSD"
	}
	return runtime.GOOS
}

// --- status, logs ----------------------------------------------------------

func (w *Worker) note(e Entry, op string, ok bool) {
	w.mu.Lock()
	defer w.mu.Unlock()
	w.requests++
	w.last = pyObj{{"op", op}, {"at", time.Now().Unix()}, {"alias", e.Alias}, {"ok", ok}}
}

// writeStatus writes worker.json, which the owner's app reads. Called
// with w.mu held.
func (w *Worker) writeStatus() error {
	var npub any
	if n, err := w.agent.Npub(); err == nil {
		npub = n
	}
	contacts, _ := w.agent.Contacts()
	var invite any
	if w.invite != nil {
		invite = pyObj{{"code", w.invite.Code}, {"link", w.invite.Link}, {"expires_at", w.invite.ExpiresAt},
			{"message", pairingMessage(w.invite.Code, w.agent.Alias(), w.agent.Card(), w.windows)}}
	}
	phase := "running"
	if w.paused() {
		phase = "paused"
	}
	err := w.st.Put("worker", pyObj{
		{"pid", os.Getpid()}, {"started", w.started}, {"alias", w.agent.Alias()}, {"npub", npub},
		{"phase", phase}, {"contacts", len(contacts)}, {"invite", invite}, {"paused", w.paused()},
		{"requests", w.requests}, {"last", orNull(w.last)}, {"paired", orNull(w.paired)}, {"work", w.work},
		{"updated", time.Now().Unix()},
	})
	w.statusWritten = time.Now()
	return err
}

// log appends one line to worker.log: every request and decision,
// pairings, unpairing.
func (w *Worker) log(record pyObj) {
	if w.logFile == "" {
		return
	}
	line := pyDumps(record.with("at", time.Now().Unix()), true) + "\n"
	f, err := os.OpenFile(w.logFile, os.O_WRONLY|os.O_CREATE|os.O_APPEND, 0o666)
	if err == nil {
		_, err = f.WriteString(line)
		if cerr := f.Close(); err == nil {
			err = cerr
		}
	}
	if err != nil {
		w.say(fmt.Sprintf("could not write worker.log: %v", err))
	}
}

// record keeps one JSON file per request in <home>/requests, updated as it
// progresses (decision, then outcome and output), for the owner's app to
// list. Fields with a nil value are left as they are. The newest
// requestsKeep are kept.
func (w *Worker) record(rid string, fields pyObj) {
	if w.requestsDir == "" || !validID(rid) {
		return // an id that isn't a plain name was refused; worker.log has it
	}
	name := rid
	err := func() error {
		if err := os.MkdirAll(w.requestsDir, 0o777); err != nil {
			return err
		}
		path := filepath.Join(w.requestsDir, name+".json")
		current := map[string]any{}
		if raw, err := os.ReadFile(path); err != nil || decodeNumbers(raw, &current) != nil || current == nil {
			current = map[string]any{"id": rid}
		}
		for _, kv := range fields {
			if kv.V != nil {
				current[kv.K] = kv.V
			}
		}
		tmp := filepath.Join(w.requestsDir, name+".tmp")
		if err := os.WriteFile(tmp, []byte(pyDumps(current, true)), 0o666); err != nil {
			return err
		}
		if err := os.Rename(tmp, path); err != nil {
			return err
		}
		return pruneRecords(w.requestsDir, requestsKeep)
	}()
	if err != nil {
		w.say(fmt.Sprintf("could not record request %s: %v", rid, err))
	}
}

func decodeNumbers(raw []byte, v any) error {
	dec := json.NewDecoder(strings.NewReader(string(raw)))
	dec.UseNumber()
	return dec.Decode(v)
}

// pruneRecords keeps the newest keep *.json files in dir.
func pruneRecords(dir string, keep int) error {
	paths, err := filepath.Glob(filepath.Join(dir, "*.json"))
	if err != nil || len(paths) <= keep {
		return err
	}
	type rec struct {
		path string
		at   time.Time
	}
	var recs []rec
	for _, p := range paths {
		if info, err := os.Stat(p); err == nil {
			recs = append(recs, rec{p, info.ModTime()})
		}
	}
	sort.SliceStable(recs, func(i, j int) bool {
		if !recs[i].at.Equal(recs[j].at) {
			return recs[i].at.Before(recs[j].at)
		}
		return recs[i].path < recs[j].path
	})
	for len(recs) > keep {
		os.Remove(recs[0].path)
		recs = recs[1:]
	}
	return nil
}

func (w *Worker) say(text string) {
	w.sayMu.Lock()
	defer w.sayMu.Unlock()
	fmt.Fprintln(w.out, text)
}

// pairingMessage is one sentence the owner pastes to their agent along
// with the code, so the agent knows what it is pairing with and what to do
// next (an agent that only gets a code may guess, for instance that it
// should become a worker itself). The owner's description of the worker,
// when set, goes in too: it is what the agent should remember the worker
// by. On Windows it says the worker takes PowerShell commands.
func pairingMessage(code, alias, about string, windows bool) string {
	desc := ""
	if about != "" {
		desc = fmt.Sprintf(` It describes itself: "%s".`, about)
	}
	msg := fmt.Sprintf(`Pair with my worker "%s" (an environment I set up for you, not something to run yourself): `+
		`accept the pairing code %s with relationship other and sharing "my own worker; run commands `+
		`there for me".%s Then send it the message help, and read the Workers section of `+
		`https://myoushq.com/skill.md before using it.`, alias, code, desc)
	if windows {
		msg += " It runs on Windows and takes PowerShell commands."
	}
	return msg
}

func nullable(s string) any {
	if s == "" {
		return nil
	}
	return s
}

func orNull(o pyObj) any {
	if o == nil {
		return nil
	}
	return o
}

// --- paths, as Python's pathlib sees them ---------------------------------

// expandUser is Path.expanduser: ~ and ~/... are the home directory,
// ~name/... that user's (on POSIX).
func expandUser(p string) string {
	if !strings.HasPrefix(p, "~") {
		return p
	}
	name, rest, _ := strings.Cut(p[1:], "/")
	if isWindows {
		if i := strings.IndexAny(p[1:], `/\`); i >= 0 {
			name, rest = p[1:1+i], p[2+i:]
		} else {
			name, rest = p[1:], ""
		}
	}
	var home string
	if name == "" {
		h, err := os.UserHomeDir()
		if err != nil {
			return p
		}
		home = h
	} else if isWindows {
		h, err := os.UserHomeDir()
		if err != nil {
			return p
		}
		home = filepath.Join(filepath.Dir(h), name)
	} else {
		u, err := user.Lookup(name)
		if err != nil {
			return p
		}
		home = u.HomeDir
	}
	if rest == "" {
		return home
	}
	return filepath.Join(home, rest)
}

// isAbsolute is PurePath.is_absolute: on Windows, a drive and a root (or
// a UNC share).
func isAbsolute(p string, windows bool) bool {
	if windows {
		return filepath.IsAbs(p)
	}
	return strings.HasPrefix(p, "/")
}

// purePath is str(Path(p)): repeated separators and "." parts removed, as
// is a trailing separator (".." stays).
func purePath(p string, windows bool) string {
	sep := "/"
	if windows {
		sep = `\`
		p = strings.ReplaceAll(p, "/", `\`)
	}
	vol := filepath.VolumeName(p)
	rest := p[len(vol):]
	lead := ""
	if strings.HasPrefix(rest, sep) {
		lead = sep
		if !windows && strings.HasPrefix(rest, "//") && !strings.HasPrefix(rest, "///") {
			lead = "//" // POSIX keeps exactly two
		}
	}
	var parts []string
	for _, part := range strings.Split(rest, sep) {
		if part != "" && part != "." {
			parts = append(parts, part)
		}
	}
	return vol + lead + strings.Join(parts, sep)
}

// joinUnder is work / p for a relative p, as pathlib joins: on Windows a
// path with a root but no drive takes the work directory's drive, and one
// with another drive is that drive's.
func joinUnder(work, p string, windows bool) string {
	if !windows {
		return work + "/" + p
	}
	vol := filepath.VolumeName(p)
	switch {
	case vol == "" && (strings.HasPrefix(p, `\`) || strings.HasPrefix(p, "/")):
		return filepath.VolumeName(work) + p
	case vol != "" && !strings.EqualFold(vol, filepath.VolumeName(work)):
		if abs, err := filepath.Abs(p); err == nil {
			return abs
		}
		return p
	case vol != "":
		return work + `\` + p[len(vol):]
	}
	return work + `\` + p
}

// within is whether p is dir or under it (case-insensitively on Windows).
func within(p, dir string, windows bool) bool {
	prefix := dir
	sep := string(filepath.Separator)
	if !strings.HasSuffix(prefix, sep) {
		prefix += sep
	}
	if windows {
		return strings.EqualFold(p, dir) || (len(p) > len(prefix) && strings.EqualFold(p[:len(prefix)], prefix))
	}
	return p == dir || strings.HasPrefix(p, prefix)
}

// realPath is Path.resolve() (os.path.realpath, not strict): symlinks
// resolved as they are met, ".." applied to what a link points at, and the
// part that doesn't exist yet kept as written.
func realPath(p string) string {
	if isWindows {
		return realPathWindows(p)
	}
	path, _ := joinRealPath("", p, map[string]*string{})
	if path == "" {
		path = "."
	}
	if abs, err := filepath.Abs(path); err == nil {
		return abs
	}
	return filepath.Clean(path)
}

// joinRealPath is posixpath._joinrealpath (Python 3.10 and later).
func joinRealPath(path, rest string, seen map[string]*string) (string, bool) {
	if strings.HasPrefix(rest, "/") {
		rest, path = rest[1:], "/"
	}
	for rest != "" {
		var name string
		name, rest, _ = strings.Cut(rest, "/")
		if name == "" || name == "." {
			continue
		}
		if name == ".." {
			if path == "" {
				path = ".."
				continue
			}
			var base string
			path, base = posixSplit(path)
			if base == ".." {
				path = posixJoin(posixJoin(path, ".."), "..")
			}
			continue
		}
		next := posixJoin(path, name)
		info, err := os.Lstat(next)
		if err != nil || info.Mode()&os.ModeSymlink == 0 {
			path = next
			continue
		}
		if resolved, ok := seen[next]; ok {
			if resolved != nil {
				path = *resolved // already resolved
				continue
			}
			return posixJoin(next, rest), false // a loop: stop here
		}
		seen[next] = nil
		target, err := os.Readlink(next)
		if err != nil {
			path = next
			continue
		}
		var ok bool
		if path, ok = joinRealPath(path, target, seen); !ok {
			return posixJoin(path, rest), false
		}
		resolved := path
		seen[next] = &resolved
	}
	return path, true
}

func posixJoin(a, b string) string {
	switch {
	case strings.HasPrefix(b, "/"):
		return b
	case a == "" || strings.HasSuffix(a, "/"):
		return a + b
	}
	return a + "/" + b
}

// posixSplit is posixpath.split.
func posixSplit(p string) (string, string) {
	i := strings.LastIndex(p, "/") + 1
	head, tail := p[:i], p[i:]
	if head != "" && strings.Trim(head, "/") != "" {
		head = strings.TrimRight(head, "/")
	}
	return head, tail
}

// realPathWindows resolves the part of p that exists and keeps the rest.
func realPathWindows(p string) string {
	p = filepath.Clean(p)
	existing, rest := p, []string{}
	for {
		if _, err := os.Lstat(existing); err == nil {
			break
		}
		parent := filepath.Dir(existing)
		if parent == existing {
			return p
		}
		rest = append([]string{filepath.Base(existing)}, rest...)
		existing = parent
	}
	resolved, err := filepath.EvalSymlinks(existing)
	if err != nil {
		resolved = existing
	}
	return filepath.Join(append([]string{resolved}, rest...)...)
}

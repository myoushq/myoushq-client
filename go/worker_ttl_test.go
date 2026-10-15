package myous

// A request's ttl (protocol.md 7.1): a request sent longer ago than its
// ttl is answered "expired" and neither reviewed nor run; the Python
// worker's tests check the same.

import (
	"os"
	"path/filepath"
	"sort"
	"strings"
	"testing"
	"time"
)

// sentAgo is an entry sent seconds ago (as the rumor's created_at says).
func sentAgo(e Entry, seconds int64) Entry {
	e.SentAt = time.Now().Unix() - seconds
	return e
}

func execEntry(cmd string, extra map[string]any) Entry {
	req := map[string]any{"myous": "exec", "id": testRID, "cmd": cmd}
	for k, v := range extra {
		req[k] = v
	}
	return request(req)
}

func putEntry(rid, path string, ttl ...string) Entry {
	return Entry{Type: "file", Direction: "in", Peer: testPeer, Alias: "muse", Name: "in.txt", Size: 24,
		X: "blobx", W: append([]string{"put", rid, path}, ttl...), Seq: 2}
}

func (f *workerFixture) record(rid string) map[string]any {
	return f.readJSON(filepath.Join(f.home, "requests", rid+".json"))
}

func keys(m map[string]any) string {
	var ks []string
	for k := range m {
		ks = append(ks, k)
	}
	sort.Strings(ks)
	return strings.Join(ks, " ")
}

func TestWorkerExpiredRequestsAreNeitherReviewedNorRun(t *testing.T) {
	f := newWorkerFixture(t)
	// The built-in review at "all" asks the owner about every request: an
	// expired one must not get that far (no question, no wait).
	os.WriteFile(filepath.Join(f.home, "review.json"), []byte(`{"level":"all"}`), 0o600)
	f.w.review.wait = 2 * time.Second
	started := time.Now()

	f.w.handle(f.ctx, sentAgo(execEntry("touch ran", map[string]any{"ttl": 150}), 1500))
	want := `{"myous": "ack", "id": "` + testRID + `", "ok": false, "error": "expired: sent 25 minutes ago, its ttl is 150 seconds; not run"}`
	if got := f.lastText(); got != want {
		t.Fatalf("exec\n got %s\nwant %s", got, want)
	}
	rec := f.record(testRID)
	if rec["decision"] != "expired" || rec["reason"] != "expired: sent 25 minutes ago, its ttl is 150 seconds; not run" ||
		rec["op"] != "exec" || rec["cmd"] != "touch ran" || rec["alias"] != "muse" || rec["sender"] != testPeer ||
		rec["at"] != rec["done_at"] || rec["at"].(float64) < float64(started.Unix()) {
		t.Fatalf("record: %v", rec)
	}
	if got := keys(rec); got != "alias at cmd decision done_at id op reason sender" {
		t.Fatalf("record keys: %s", got)
	}
	log := f.logLines()
	if l := log[len(log)-1]; l["op"] != "exec" || l["id"] != testRID || l["sender"] != testPeer || l["decision"] != "expired" ||
		l["reason"] != rec["reason"] {
		t.Fatalf("log: %v", l)
	}

	get := "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"
	os.WriteFile(filepath.Join(f.work, "there.txt"), []byte("x"), 0o600)
	f.w.handle(f.ctx, sentAgo(request(map[string]any{"myous": "get", "id": get, "path": "there.txt", "ttl": 60}), 3*3600+5))
	if r := f.last(); r["ok"] != false || r["id"] != get || r["error"] != "expired: sent 3 hours ago, its ttl is 60 seconds; not run" {
		t.Fatalf("get: %v", r)
	}
	if len(f.agent.files) != 0 {
		t.Fatalf("an expired get sent %v", f.agent.files)
	}
	if rec := f.record(get); rec["decision"] != "expired" || rec["path"] != "there.txt" || rec["op"] != "get" {
		t.Fatalf("get record: %v", rec)
	}

	// An expired put is answered before its file is downloaded.
	put := "efefefefefefefefefefefefefefefef"
	f.agent.blobs["blobx"] = []byte("contents")
	f.w.handle(f.ctx, sentAgo(putEntry(put, "in.txt", "30"), 150))
	if r := f.last(); r["ok"] != false || r["id"] != put || r["error"] != "expired: sent 2 minutes ago, its ttl is 30 seconds; not run" {
		t.Fatalf("put: %v", r)
	}
	if f.agent.fetches != 0 || exists(filepath.Join(f.work, "in.txt")) {
		t.Fatalf("an expired put was fetched (%d) or written", f.agent.fetches)
	}
	rec = f.record(put)
	if got := keys(rec); got != "alias at decision done_at id op path reason sender size" || rec["size"] != 24.0 || rec["path"] != "in.txt" {
		t.Fatalf("put record: %v", rec)
	}

	if exists(filepath.Join(f.work, "ran")) || exists(filepath.Join(f.home, "approvals")) || time.Since(started) > 2*time.Second {
		t.Fatal("an expired request ran, or was put to the owner")
	}
	// Not a refusal: the owner's app notifies those.
	for _, l := range f.logLines() {
		if d, _ := l["decision"].(string); d != "expired" {
			t.Fatalf("log: %v", l)
		}
	}
	f.writeStatus()
	if s := f.status(); s["requests"] != 3.0 {
		t.Fatalf("status: %v", s)
	}
}

func TestWorkerExpiredRequestSkipsTheReviewHook(t *testing.T) {
	needPOSIX(t)
	f := newWorkerFixture(t)
	hook := filepath.Join(f.tmp, "review.sh")
	seen := filepath.Join(f.tmp, "seen")
	os.WriteFile(hook, []byte("touch "+seen+"\nexit 0\n"), 0o700)
	f.w.reviewCmd = "/bin/sh " + hook
	f.w.handle(f.ctx, sentAgo(execEntry("touch ran", nil), 700))
	if r := f.last(); r["error"] != "expired: sent 11 minutes ago, its ttl is 600 seconds; not run" {
		t.Fatalf("%v", r)
	}
	if exists(seen) || exists(filepath.Join(f.work, "ran")) {
		t.Fatal("the hook saw it, or it ran")
	}
	// A fresh one goes through the hook and runs.
	if r := f.exec("touch ran"); r["exit"] != 0.0 || !exists(seen) || !exists(filepath.Join(f.work, "ran")) {
		t.Fatalf("%v", r)
	}
}

func TestWorkerExpiredText(t *testing.T) {
	for age, want := range map[int64]string{
		59:    "expired: sent 59 seconds ago, its ttl is 30 seconds; not run",
		119:   "expired: sent 119 seconds ago, its ttl is 30 seconds; not run",
		120:   "expired: sent 2 minutes ago, its ttl is 30 seconds; not run",
		150:   "expired: sent 2 minutes ago, its ttl is 30 seconds; not run",
		1500:  "expired: sent 25 minutes ago, its ttl is 30 seconds; not run",
		7199:  "expired: sent 119 minutes ago, its ttl is 30 seconds; not run",
		7200:  "expired: sent 2 hours ago, its ttl is 30 seconds; not run",
		10800: "expired: sent 3 hours ago, its ttl is 30 seconds; not run",
		90000: "expired: sent 25 hours ago, its ttl is 30 seconds; not run",
	} {
		if got := expiredWhy(age, 30); got != want {
			t.Errorf("%d s: %s", age, got)
		}
	}
	if got := expiredWhy(1500, 150); got != "expired: sent 25 minutes ago, its ttl is 150 seconds; not run" {
		t.Error(got)
	}
	if got := expiredWhy(2, 1); got != "expired: sent 2 seconds ago, its ttl is 1 second; not run" {
		t.Error(got)
	}
	now := time.Unix(1_000_000, 900_000_000)
	for sentAt, want := range map[int64]int64{1_000_000 - 59: 59, 1_000_000: 0, 1_000_000 + 30: 0} {
		if got := requestAge(sentAt, now); got != want {
			t.Errorf("age of %d: %d", sentAt, got)
		}
	}
}

func TestWorkerRequestTTLs(t *testing.T) {
	needPOSIX(t)
	f := newWorkerFixture(t)
	runs := func(e Entry) bool {
		t.Helper()
		os.Remove(filepath.Join(f.work, "ran"))
		f.w.handle(f.ctx, e)
		return exists(filepath.Join(f.work, "ran"))
	}
	// Without a ttl (or with a null one): 600 seconds.
	if !runs(sentAgo(execEntry("touch ran", nil), 500)) {
		t.Fatalf("500 s old, default ttl: %v", f.last())
	}
	if runs(sentAgo(execEntry("touch ran", nil), 660)) || f.last()["error"] != "expired: sent 11 minutes ago, its ttl is 600 seconds; not run" {
		t.Fatalf("660 s old, default ttl: %v", f.last())
	}
	if runs(sentAgo(execEntry("touch ran", map[string]any{"ttl": nil}), 660)) || f.last()["error"] != "expired: sent 11 minutes ago, its ttl is 600 seconds; not run" {
		t.Fatalf("660 s old, null ttl: %v", f.last())
	}
	if runs(sentAgo(putEntry(testRID, "ran"), 660)) || f.last()["error"] != "expired: sent 11 minutes ago, its ttl is 600 seconds; not run" {
		t.Fatalf("put 660 s old, default ttl: %v", f.last())
	}
	// A longer ttl leaves it for a worker that was off; above a day counts as a day.
	if !runs(sentAgo(execEntry("touch ran", map[string]any{"ttl": 7200}), 3600)) {
		t.Fatalf("ttl 7200: %v", f.last())
	}
	if !runs(sentAgo(execEntry("touch ran", map[string]any{"ttl": 1e9}), 86000)) {
		t.Fatalf("ttl 1e9, a day old: %v", f.last())
	}
	if runs(sentAgo(execEntry("touch ran", map[string]any{"ttl": 1e9}), 90000)) ||
		f.last()["error"] != "expired: sent 25 hours ago, its ttl is 86400 seconds; not run" {
		t.Fatalf("ttl 1e9, 25 hours old: %v", f.last())
	}
	if runs(sentAgo(textEntry(`{"myous": "exec", "id": "`+testRID+`", "cmd": "touch ran", "ttl": 1`+strings.Repeat("0", 400)+`}`), 90000)) ||
		!strings.HasSuffix(f.last()["error"].(string), "its ttl is 86400 seconds; not run") {
		t.Fatalf("a 401-digit ttl: %v", f.last())
	}
	// Fractions count in whole seconds.
	if runs(sentAgo(execEntry("touch ran", map[string]any{"ttl": 150.9}), 1500)) ||
		f.last()["error"] != "expired: sent 25 minutes ago, its ttl is 150 seconds; not run" {
		t.Fatalf("ttl 150.9: %v", f.last())
	}
	// Without a send time (an entry from before sent_at was kept) nothing expires.
	if !runs(execEntry("touch ran", map[string]any{"ttl": 1})) {
		t.Fatalf("no send time: %v", f.last())
	}
	// A send time ahead of the worker's clock counts as now.
	future := execEntry("touch ran", map[string]any{"ttl": 1})
	future.SentAt = time.Now().Unix() + 3600
	if !runs(future) {
		t.Fatalf("sent in the future: %v", f.last())
	}
}

func TestWorkerBadTTLsAreRefused(t *testing.T) {
	needPOSIX(t)
	f := newWorkerFixture(t)
	refused := func(what string, e Entry) {
		t.Helper()
		f.w.handle(f.ctx, e)
		if r := f.last(); r["ok"] != false || r["error"] != badTTL || exists(filepath.Join(f.work, "ran")) {
			t.Fatalf("%s: %v", what, r)
		}
	}
	for _, ttl := range []any{true, false, "600", 0, -5, 0.5, []int{600}, map[string]any{}} {
		refused("ttl "+pyDumps(ttl, false), execEntry("touch ran", map[string]any{"ttl": ttl}))
	}
	for _, ttl := range []string{"NaN", "Infinity", "-Infinity", "1e400", "-1e400"} {
		refused("ttl "+ttl, textEntry(`{"myous": "exec", "id": "`+testRID+`", "cmd": "touch ran", "ttl": `+ttl+`}`))
	}
	refused("get", request(map[string]any{"myous": "get", "id": testRID, "path": "x", "ttl": "x"}))
	f.agent.blobs["blobx"] = []byte("contents")
	for _, ttl := range []string{"", "0", "000", "0.5", "1.5", "60.0", "-1", "+60", "1e3", " 60", "60 ", "6 0", "60.", ".5", "NaN", "inf", "abc", "١٢"} {
		refused("put ttl "+ttl, putEntry(testRID, "ran", ttl))
	}
	if f.agent.fetches != 0 {
		t.Fatalf("%d fetches", f.agent.fetches)
	}
	// Good ones, in the w tag and in JSON.
	for ttl, want := range map[string]int64{"1": 1, "600": 600, "0090": 90, "86401": 86400, strings.Repeat("9", 400): 86400} {
		if got, ok := tagTTL([]string{"put", testRID, "x", ttl}); !ok || got != want {
			t.Errorf("tag ttl %q: %d %v", ttl, got, ok)
		}
	}
	if got, ok := tagTTL([]string{"put", testRID, "x"}); !ok || got != requestTTL {
		t.Errorf("no tag ttl: %d %v", got, ok)
	}
	for text, want := range map[string]int64{"1": 1, "600": 600, "600.7": 600, "1e3": 1000, "86400.5": 86400, "1e9": 86400} {
		v, _ := pyLoads(text)
		if got, ok := jsonTTL(v, true); !ok || got != want {
			t.Errorf("JSON ttl %s: %d %v", text, got, ok)
		}
	}
	f.w.handle(f.ctx, putEntry(testRID, "ran", "60"))
	if r := f.last(); r["ok"] != true || !exists(filepath.Join(f.work, "ran")) {
		t.Fatalf("put with ttl 60: %v", r)
	}
}

func TestWorkerStaleTextGetsNoHelp(t *testing.T) {
	f := newWorkerFixture(t)
	// Sent while the worker was off, long enough ago that whoever asked has moved on.
	f.w.handle(f.ctx, sentAgo(textEntry("help"), 700))
	if n := f.agent.sentCount(); n != 0 {
		t.Fatalf("%d replies to a stale help", n)
	}
	f.w.handle(f.ctx, sentAgo(textEntry("help"), 500))
	if n := f.agent.sentCount(); n != 1 || !strings.Contains(f.lastText(), "myoushq worker") {
		t.Fatalf("%d replies to a fresh help", n)
	}
}

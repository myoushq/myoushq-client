package myous

import (
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/nbd-wtf/go-nostr"
)

// claimHub is a fake hub with the claim rules (a repeat with the
// claimer's id gets the same token; anything else after the claim gets
// 409) and a mailbox that takes our messages and has none for us. It loses
// the first `lose` claim answers after acting on them, and answers the
// statuses in `fail` before acting.
type claimHub struct {
	mu        sync.Mutex
	lose      int
	fail      []int
	claimedBy string
	claimedAt int64
	claims    []string
	posts     int
}

const testNameplate = "4821"

func (h *claimHub) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	h.mu.Lock()
	defer h.mu.Unlock()
	reply := func(status int, v any) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(status)
		json.NewEncoder(w).Encode(v)
	}
	switch {
	case r.Method == "POST" && r.URL.Path == "/api/pair/"+testNameplate+"/claim":
		var req struct {
			Claim string `json:"claim"`
		}
		json.NewDecoder(r.Body).Decode(&req)
		h.claims = append(h.claims, req.Claim)
		if len(h.fail) > 0 {
			status := h.fail[0]
			h.fail = h.fail[1:]
			reply(status, map[string]string{"error": http.StatusText(status)})
			return
		}
		answer := map[string]any{"repeat": h.claimedBy != ""}
		if h.claimedBy == "" {
			h.claimedBy, h.claimedAt = req.Claim, time.Now().Unix()-5
		} else if req.Claim != h.claimedBy {
			reply(409, map[string]string{"error": "invite already used, ask for a new one"})
			return
		} else {
			answer["claimed_at"] = h.claimedAt
		}
		if h.lose > 0 {
			h.lose--
			conn, _, _ := w.(http.Hijacker).Hijack()
			conn.Close() // the claim is taken; its answer never arrives
			return
		}
		answer["token"], answer["expires_at"] = "tok-"+h.claimedBy, time.Now().Unix()+900
		reply(200, answer)
	case r.Method == "POST" && r.URL.Path == "/api/pair/"+testNameplate+"/messages":
		if r.Header.Get("Authorization") != "Bearer tok-"+h.claimedBy {
			reply(403, map[string]string{"error": "bad token"})
			return
		}
		h.posts++
		reply(200, map[string]int{"index": h.posts - 1})
	case r.Method == "GET" && r.URL.Path == "/api/pair/"+testNameplate+"/messages":
		reply(200, map[string]any{"messages": []string{}, "next": 0})
	default:
		reply(404, map[string]string{"error": "unexpected " + r.Method + " " + r.URL.Path})
	}
}

// got returns the claim ids sent and the messages posted so far.
func (h *claimHub) got() ([]string, int) {
	h.mu.Lock()
	defer h.mu.Unlock()
	return append([]string(nil), h.claims...), h.posts
}

type claimFixture struct {
	t   *testing.T
	st  *memStorage
	hub *claimHub
	url string
}

func newClaimFixture(t *testing.T, hub *claimHub) *claimFixture {
	old := claimRetryDelay
	claimRetryDelay = time.Millisecond
	t.Cleanup(func() { claimRetryDelay = old })
	srv := httptest.NewServer(hub)
	t.Cleanup(srv.Close)
	st := &memStorage{docs: map[string][]byte{}}
	st.Put("hub", HubConfig{PairLinkBase: "https://hub.test/p/", URL: srv.URL, FetchedAt: time.Now().Unix()})
	return &claimFixture{t: t, st: st, hub: hub, url: srv.URL}
}

func (f *claimFixture) accept() (*Pending, error) {
	pr := &pairing{st: f.st, hub: newHub(f.st, f.url), alias: "me"}
	return pr.accept(f.t.Context(), testNameplate+"-K7F3QX", 0, ContactContext{})
}

func (f *claimFixture) savedClaim() string {
	var c claiming
	f.st.Get("claiming/"+testNameplate, &c)
	return c.Claim
}

func claims(f *claimFixture) []string { c, _ := f.hub.got(); return c }
func posts(f *claimFixture) int       { _, n := f.hub.got(); return n }
func claimedAt(f *claimFixture) int64 {
	f.hub.mu.Lock()
	defer f.hub.mu.Unlock()
	return f.hub.claimedAt
}

func TestClaimLostAnswerThenRetry(t *testing.T) {
	f := newClaimFixture(t, &claimHub{lose: 1})
	p, err := f.accept()
	if err != nil {
		t.Fatal(err)
	}
	c := claims(f)
	if len(c) != 2 || c[0] != c[1] || !regexp.MustCompile(`^[0-9a-f]{32}$`).MatchString(c[0]) {
		t.Fatalf("claims sent: %q", c)
	}
	if p.Stage != "wait_pake" || p.Token != "tok-"+c[0] || posts(f) != 1 {
		t.Fatalf("pending %+v, %d posts", p, posts(f))
	}
	if f.savedClaim() != "" {
		t.Fatal("the claiming record outlived the pairing record")
	}
}

func TestClaimProxyErrorIsRetried(t *testing.T) {
	f := newClaimFixture(t, &claimHub{fail: []int{502}})
	if _, err := f.accept(); err != nil {
		t.Fatal(err)
	}
	if len(claims(f)) != 2 {
		t.Fatalf("claims sent: %q", claims(f))
	}
}

func TestClaimRerunAfterCrashReusesID(t *testing.T) {
	id := strings.Repeat("ab", 16)
	f := newClaimFixture(t, &claimHub{claimedBy: id})
	// The first run saved its id and died before it saw the hub's answer.
	f.st.Put("claiming/"+testNameplate, claiming{Nameplate: testNameplate, Claim: id})
	p, err := f.accept()
	if err != nil {
		t.Fatal(err)
	}
	if len(claims(f)) != 1 || claims(f)[0] != id || p.Token != "tok-"+id {
		t.Fatalf("claims %q, token %q", claims(f), p.Token)
	}
	if f.savedClaim() != "" {
		t.Fatal("the claiming record outlived the pairing record")
	}
}

func TestClaimUnreachableKeepsIDForNextRun(t *testing.T) {
	f := newClaimFixture(t, &claimHub{lose: claimAttempts})
	_, err := f.accept()
	var pe *PairingError
	if err == nil || errors.As(err, &pe) || !strings.Contains(err.Error(), "could not reach the hub") ||
		!strings.Contains(err.Error(), "EOF") {
		t.Fatalf("got %v; want a network error that names the drop", err)
	}
	if len(claims(f)) != claimAttempts || f.savedClaim() != claims(f)[0] {
		t.Fatalf("claims %q, saved %q", claims(f), f.savedClaim())
	}
	// The same accept again carries on with the claim the hub already gave.
	p, err := f.accept()
	if err != nil {
		t.Fatal(err)
	}
	for _, c := range claims(f) {
		if c != claims(f)[0] {
			t.Fatalf("a new id on the re-run: %q", claims(f))
		}
	}
	if p.Token != "tok-"+claims(f)[0] || f.savedClaim() != "" {
		t.Fatalf("token %q, saved %q", p.Token, f.savedClaim())
	}
}

func TestClaimTakenBySomeoneElse(t *testing.T) {
	f := newClaimFixture(t, &claimHub{claimedBy: strings.Repeat("cd", 16)})
	_, err := f.accept()
	var pe *PairingError
	if !errors.As(err, &pe) || pe.Msg != "invite already used, ask for a new one" {
		t.Fatalf("got %v", err)
	}
	if len(claims(f)) != 1 || f.savedClaim() != "" {
		t.Fatalf("claims %q, saved %q: a refusal is final and drops the record", claims(f), f.savedClaim())
	}
}

func TestClaimRateLimitKeepsID(t *testing.T) {
	f := newClaimFixture(t, &claimHub{fail: []int{429}})
	if _, err := f.accept(); err == nil {
		t.Fatal("a 429 succeeded")
	}
	if len(claims(f)) != 1 || f.savedClaim() != claims(f)[0] {
		t.Fatalf("claims %q, saved %q", claims(f), f.savedClaim())
	}
}

func TestClaimTraceNamesTheDropAndTheResume(t *testing.T) {
	f := newClaimFixture(t, &claimHub{lose: 1})
	p, err := f.accept()
	if err != nil {
		t.Fatal(err)
	}
	var steps []string
	for _, s := range p.Trace {
		steps = append(steps, s.Step)
	}
	if strings.Join(steps, ",") != "claim,claim,post" {
		t.Fatalf("steps %v", steps)
	}
	host := strings.TrimPrefix(f.url, "http://")
	first, second := p.Trace[0], p.Trace[1]
	if first.Error == "" || !strings.Contains(first.Error, "EOF") || first.Host != host {
		t.Errorf("the drop: %+v", first)
	}
	if second.Error != "" || second.ClaimedAt != claimedAt(f) {
		t.Errorf("the resume: %+v (claimed at %d)", second, claimedAt(f))
	}
	var stored Pending
	f.st.Get("pending/"+testNameplate, &stored)
	if len(stored.Trace) != 3 {
		t.Errorf("the pairing record holds %d steps", len(stored.Trace))
	}
}

func TestFailedAcceptKeepsTheTrace(t *testing.T) {
	f := newClaimFixture(t, &claimHub{claimedBy: strings.Repeat("cd", 16)})
	started := time.Now().Unix()
	if _, err := f.accept(); err == nil {
		t.Fatal("accepted a taken code")
	}
	agent := &Agent{st: f.st}
	nameplate, steps := agent.PairingTrace(started)
	if nameplate != testNameplate || len(steps) != 1 || steps[0].Step != "claim" ||
		!strings.Contains(steps[0].Error, "invite already used") {
		t.Fatalf("trace %q %+v", nameplate, steps)
	}
	if _, later := agent.PairingTrace(started + 3600); later != nil {
		t.Fatal("an older trace counted as this run's")
	}
}

// mailboxHub is a fake hub with both sides of one mailbox, as the hub keeps
// it: a claim id that repeats, and a side's repeat of its last message
// answered without appending. The answers to the first losePosts of side
// B's posts are lost after the message is stored.
type mailboxHub struct {
	mu        sync.Mutex
	losePosts int
	claimedBy string
	messages  map[string][]string
}

func (h *mailboxHub) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	h.mu.Lock()
	defer h.mu.Unlock()
	reply := func(status int, v any) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(status)
		json.NewEncoder(w).Encode(v)
	}
	side := map[string]string{"Bearer tok-a": "a", "Bearer tok-b": "b"}[r.Header.Get("Authorization")]
	box := "/api/pair/" + testNameplate
	exp := time.Now().Unix() + 900
	switch {
	case r.Method == "POST" && r.URL.Path == "/api/pair":
		reply(201, map[string]any{"nameplate": testNameplate, "token": "tok-a", "expires_at": exp})
	case r.Method == "POST" && r.URL.Path == box+"/claim":
		var req struct{ Claim string }
		json.NewDecoder(r.Body).Decode(&req)
		if h.claimedBy != "" && h.claimedBy != req.Claim {
			reply(409, map[string]string{"error": "invite already used, ask for a new one"})
			return
		}
		repeat := h.claimedBy != ""
		h.claimedBy = req.Claim
		reply(200, map[string]any{"token": "tok-b", "expires_at": exp, "repeat": repeat})
	case r.Method == "POST" && r.URL.Path == box+"/messages":
		var req struct{ Body string }
		json.NewDecoder(r.Body).Decode(&req)
		mine := h.messages[side]
		if n := len(mine); n > 0 && mine[n-1] == req.Body {
			reply(200, map[string]any{"index": n - 1, "repeat": true})
			return
		}
		h.messages[side] = append(mine, req.Body)
		if side == "b" && h.losePosts > 0 {
			h.losePosts--
			conn, _, _ := w.(http.Hijacker).Hijack()
			conn.Close()
			return
		}
		reply(200, map[string]any{"index": len(mine)})
	case r.Method == "GET" && r.URL.Path == box+"/messages":
		after, _ := strconv.Atoi(r.URL.Query().Get("after"))
		other := h.messages[map[string]string{"a": "b", "b": "a"}[side]]
		msgs := []string{}
		if after < len(other) {
			msgs = other[after:]
		}
		reply(200, map[string]any{"messages": msgs, "next": after + len(msgs)})
	case r.Method == "DELETE":
		w.WriteHeader(204)
	default:
		reply(404, map[string]string{"error": "unexpected"})
	}
}

// sent returns the messages side has posted so far.
func (h *mailboxHub) sent(side string) []string {
	h.mu.Lock()
	defer h.mu.Unlock()
	return append([]string(nil), h.messages[side]...)
}

func TestLostPostAnswerRerunCompletesWithoutSecondPake(t *testing.T) {
	old := claimRetryDelay
	claimRetryDelay = time.Millisecond
	t.Cleanup(func() { claimRetryDelay = old })
	hub := &mailboxHub{losePosts: 1, messages: map[string][]string{}}
	srv := httptest.NewServer(hub)
	t.Cleanup(srv.Close)
	side := func(alias string) *pairing {
		st := &memStorage{docs: map[string][]byte{}}
		st.Put("hub", HubConfig{PairLinkBase: "https://hub.test/p/", URL: srv.URL, FetchedAt: time.Now().Unix()})
		sk := nostr.GeneratePrivateKey()
		pk, _ := nostr.GetPublicKey(sk)
		return &pairing{st: st, hub: newHub(st, srv.URL), sk: sk, pk: pk, alias: alias}
	}
	a, b := side("alice"), side("bob")
	ctx := t.Context()
	inv, err := a.invite(ctx, ContactContext{})
	if err != nil {
		t.Fatal(err)
	}
	// The post reaches the mailbox; its answer doesn't.
	first, err := b.accept(ctx, inv.Code, 0, ContactContext{})
	if err != nil || first.Stage != "posting" {
		t.Fatalf("first accept: %+v, %v", first, err)
	}
	var saved Pending
	b.st.Get("pending/"+testNameplate, &saved)
	if sent := hub.sent("b"); saved.Stage != "posting" || len(sent) != 1 || saved.Message != sent[0] {
		t.Fatalf("saved %q %q; the mailbox has %q", saved.Stage, saved.Message, sent)
	}
	// The same accept again.
	again, err := b.accept(ctx, inv.Code, 0, ContactContext{})
	if err != nil || again.Stage != "wait_payload" {
		t.Fatalf("re-run: %+v, %v", again, err)
	}
	var steps []string
	for _, s := range again.Trace {
		steps = append(steps, s.Step)
	}
	if strings.Join(steps, ",") != "claim,post,post (repeat)" || again.Trace[1].Error == "" {
		t.Fatalf("trace %+v", again.Trace)
	}
	doneA, err := a.advanceAll(ctx)
	if err != nil || len(doneA) != 1 || doneA[0].Stage != "done" {
		t.Fatalf("side A: %+v, %v", doneA, err)
	}
	doneB, err := b.advanceAll(ctx)
	if err != nil || len(doneB) != 1 || doneB[0].Stage != "done" || doneB[0].Verify != doneA[0].Verify {
		t.Fatalf("side B: %+v, %v", doneB, err)
	}
	pakes := 0
	for _, m := range hub.sent("b") {
		raw, _ := unb64(m)
		if strings.Contains(string(raw), `"t":"pake"`) {
			pakes++
		}
	}
	if pakes != 1 {
		t.Fatalf("%d PAKE messages from B", pakes)
	}
}

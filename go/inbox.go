package myous

import (
	"sort"
	"time"

	"github.com/nbd-wtf/go-nostr"
)

// Entry is one history record: a message ("message", direction "in" or
// "out") or a pairing result ("paired", "pairing_failed").
type Entry struct {
	Seq       int    `json:"seq"`
	Type      string `json:"type"`
	Direction string `json:"direction,omitempty"`
	Peer      string `json:"peer,omitempty"`
	Alias     string `json:"alias,omitempty"`
	Text      string `json:"text"`
	At        int64  `json:"at"`
	SentAt    int64  `json:"sent_at,omitempty"`
}

// State is the "state" document.
type State struct {
	NextSeq    int              `json:"next_seq,omitempty"`
	ReadSeq    int              `json:"read_seq,omitempty"`
	Seen       map[string]int64 `json:"seen,omitempty"`
	Registered bool             `json:"registered,omitempty"`
	LastPoll   int64            `json:"last_poll,omitempty"`
}

// Longer than relay retention plus wrap timestamp jitter.
const seenRetention = 3 * 86400

func loadState(st Storage) (State, error) {
	var s State
	_, err := st.Get("state", &s)
	return s, err
}

// record appends to the history. Callers hold the "state" lock.
func record(st Storage, e Entry) (Entry, error) {
	s, err := loadState(st)
	if err != nil {
		return e, err
	}
	if s.NextSeq == 0 {
		s.NextSeq = 1
	}
	e.Seq, e.At = s.NextSeq, time.Now().Unix()
	if err := st.AppendHistory(e); err != nil {
		return e, err
	}
	s.NextSeq++
	return e, st.Put("state", s)
}

// handleWraps stores messages from approved contacts and drops everything
// else. Callers hold the "state" lock.
func handleWraps(st Storage, sk string, wraps []*nostr.Event) ([]Entry, error) {
	s, err := loadState(st)
	if err != nil {
		return nil, err
	}
	if s.Seen == nil {
		s.Seen = map[string]int64{}
	}
	now := time.Now().Unix()
	var fresh []*nostr.Event
	for _, w := range wraps {
		if _, ok := s.Seen[w.ID]; ok {
			continue
		}
		s.Seen[w.ID] = now
		fresh = append(fresh, w)
	}
	for id, t := range s.Seen {
		if now-t >= seenRetention {
			delete(s.Seen, id)
		}
	}
	if err := st.Put("state", s); err != nil {
		return nil, err
	}

	// Wrap timestamps are randomized, so put messages in the order they were written.
	var msgs []message
	for _, w := range fresh {
		if m, ok := unwrap(sk, w); ok {
			msgs = append(msgs, m)
		}
	}
	sort.SliceStable(msgs, func(i, j int) bool {
		if msgs[i].sentAt != msgs[j].sentAt {
			return msgs[i].sentAt < msgs[j].sentAt
		}
		return msgs[i].ms < msgs[j].ms
	})

	var stored []Entry
	for _, m := range msgs {
		c, err := approvedContact(st, m.sender)
		if err != nil {
			return stored, err
		}
		if c == nil {
			continue // not paired, or blocked: drop silently
		}
		e, err := record(st, Entry{Type: "message", Direction: "in", Peer: c.Npub, Alias: c.Alias, Text: m.text, SentAt: m.sentAt})
		if err != nil {
			return stored, err
		}
		stored = append(stored, e)
	}
	return stored, nil
}

func unread(st Storage, markRead bool) ([]Entry, error) {
	unlock, _, err := lock(st, "state", true)
	if err != nil {
		return nil, err
	}
	defer unlock()
	s, err := loadState(st)
	if err != nil {
		return nil, err
	}
	history, err := st.ReadHistory()
	if err != nil {
		return nil, err
	}
	entries := []Entry{}
	for _, e := range history {
		if e.Seq > s.ReadSeq && e.Direction != "out" {
			entries = append(entries, e)
		}
	}
	if markRead && len(entries) > 0 {
		s.ReadSeq = entries[len(entries)-1].Seq
		if err := st.Put("state", s); err != nil {
			return nil, err
		}
	}
	return entries, nil
}

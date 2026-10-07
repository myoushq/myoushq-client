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
	Version   string `json:"version,omitempty"` // "update" entries: the release announced
	// The contact's current relationship context, filled in when read.
	Relationship string `json:"relationship,omitempty"`
	Sharing      string `json:"sharing,omitempty"`
	// Incomplete marks a long message whose missing parts never arrived.
	Incomplete bool   `json:"incomplete,omitempty"`
	ID         string `json:"id,omitempty"`  // "notice" entries: the notice's id
	URL        string `json:"url,omitempty"` // "notice" entries: link for more detail
}

// State is the "state" document.
type State struct {
	NextSeq    int              `json:"next_seq,omitempty"`
	ReadSeq    int              `json:"read_seq,omitempty"`
	Seen       map[string]int64 `json:"seen,omitempty"`
	Registered bool             `json:"registered,omitempty"`
	LastPoll   int64            `json:"last_poll,omitempty"`
	// AnnouncedRelease is the newest release already announced in the inbox.
	AnnouncedRelease string `json:"announced_release,omitempty"`
	// SeenNotices are the ids of hub notices already passed on.
	SeenNotices []string `json:"seen_notices,omitempty"`
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
		if msgs[i].ms != msgs[j].ms {
			return msgs[i].ms < msgs[j].ms
		}
		return msgs[i].part != nil && msgs[j].part != nil && msgs[i].part.index < msgs[j].part.index
	})

	buf := map[string]*unfinished{}
	if _, err := st.Get("partials", &buf); err != nil {
		return nil, err
	}
	changed := false
	var stored []Entry
	keep := func(m message, incomplete bool) error {
		c, err := approvedContact(st, m.sender)
		if err != nil || c == nil {
			return err // c == nil: not paired, or blocked: drop silently
		}
		if m.part != nil {
			changed = true
			whole, done := addPart(buf, m, now)
			if !done {
				return nil // waiting for the other parts
			}
			m = whole
		}
		e, err := record(st, Entry{Type: "message", Direction: "in", Peer: c.Npub, Alias: c.Alias, Text: m.text, SentAt: m.sentAt, Incomplete: incomplete})
		if err == nil {
			stored = append(stored, e)
		}
		return err
	}
	for _, m := range msgs {
		if err := keep(m, false); err != nil {
			return stored, err
		}
	}
	for _, m := range expireParts(buf, now) {
		changed = true
		if err := keep(m, true); err != nil {
			return stored, err
		}
	}
	if changed {
		if err := st.Put("partials", buf); err != nil {
			return stored, err
		}
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
			if e.Peer != "" {
				// So the agent has it when it answers.
				e.Relationship, e.Sharing = contextOf(st, e.Peer)
			}
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

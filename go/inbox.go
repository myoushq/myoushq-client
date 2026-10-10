package myous

import (
	"encoding/hex"
	"encoding/json"
	"sort"
	"strings"
	"time"

	"github.com/nbd-wtf/go-nostr"
)

// Entry is one history record: a message ("message", direction "in" or
// "out"), a file ("file"), a worker reply ("result", "ack"; see
// protocol.md section 7), a pairing result ("paired", "pairing_failed")
// or a card ("card": what a contact says about itself, or ours sent to
// it; protocol.md section 4).
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
	// The contact's current relationship context and what it says about
	// itself, filled in when read. "card" entries carry their own About.
	Relationship string `json:"relationship,omitempty"`
	Sharing      string `json:"sharing,omitempty"`
	About        string `json:"about,omitempty"`
	// Incomplete marks a long message whose missing parts never arrived.
	Incomplete bool   `json:"incomplete,omitempty"`
	ID         string `json:"id,omitempty"`  // "notice": the notice's id; "result"/"ack": the request's id
	URL        string `json:"url,omitempty"` // "notice": link for more detail; "file": the blob
	// "file" entries: what's needed to fetch and decrypt the blob. The key
	// and nonce stay in the history (private, like the messages). Name is
	// also a "card" entry's name (the sender's alias as it states it).
	Name  string   `json:"name,omitempty"`
	Mime  string   `json:"mime,omitempty"`
	Size  int64    `json:"size,omitempty"`
	X     string   `json:"x,omitempty"`
	Ox    string   `json:"ox,omitempty"`
	Key   string   `json:"key,omitempty"`
	Nonce string   `json:"nonce,omitempty"`
	W     []string `json:"w,omitempty"` // worker semantics, e.g. ["file", <id>]
}

// consumedByCommand reports entries that a blocking command (exec, cp)
// matches by id and consumes, so they don't show up as unread.
func (e Entry) consumedByCommand() bool {
	return e.Type == "result" || e.Type == "ack" || (e.Type == "file" && len(e.W) > 0 && e.W[0] == "file")
}

// workerReply recognizes a worker's JSON reply in a text message.
func workerReply(text string) (kind, id string, ok bool) {
	if !strings.HasPrefix(strings.TrimSpace(text), "{") {
		return "", "", false
	}
	var r struct {
		Myous string `json:"myous"`
		ID    string `json:"id"`
	}
	if json.Unmarshal([]byte(text), &r) != nil || (r.Myous != "result" && r.Myous != "ack") {
		return "", "", false
	}
	return r.Myous, r.ID, true
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
	// File messages must point at the hub's own blob store.
	var cfg HubConfig
	st.Get("hub", &cfg)
	changed := false
	var stored []Entry
	keep := func(m message, incomplete bool) error {
		c, err := approvedContact(st, m.sender)
		if err != nil || c == nil {
			return err // c == nil: not paired, or blocked: drop silently
		}
		if m.file != nil {
			x, ok := blobURLHash(m.file.URL, cfg.BlobAPI)
			if !ok || x != m.file.X {
				return nil // not our hub's blob, or a URL that lies about the hash: drop it
			}
			e, err := record(st, Entry{
				Type: "file", Direction: "in", Peer: c.Npub, Alias: c.Alias, SentAt: m.sentAt,
				Text: "file: " + m.file.Name, Name: m.file.Name, Mime: m.file.Mime, Size: m.file.Size,
				X: m.file.X, Ox: m.file.Ox, URL: m.file.URL, Key: hex.EncodeToString(m.file.Key),
				Nonce: hex.EncodeToString(m.file.Nonce), W: m.file.W,
			})
			if err == nil {
				stored = append(stored, e)
			}
			return err
		}
		if m.part != nil {
			changed = true
			whole, done := addPart(buf, m, now)
			if !done {
				return nil // waiting for the other parts
			}
			m = whole
		}
		entry := Entry{Type: "message", Direction: "in", Peer: c.Npub, Alias: c.Alias, Text: m.text, SentAt: m.sentAt, Incomplete: incomplete}
		if name, about, ok := parseCard(m.text); ok {
			// A card is kept on the contact; the entry says what changed.
			_, line, err := receiveCard(st, m.sender, name, about, now)
			if err != nil {
				return err
			}
			entry.Type, entry.Name, entry.About, entry.Text = "card", name, about, line
		} else if kind, id, ok := workerReply(m.text); ok {
			entry.Type, entry.ID = kind, id
		}
		e, err := record(st, entry)
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
		if e.Seq > s.ReadSeq && e.Direction != "out" && !e.consumedByCommand() {
			if e.Peer != "" {
				// So the agent has it when it answers.
				e.Relationship, e.Sharing, e.About = contextOf(st, e.Peer)
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

package myous

// Long messages: splitting text into parts and putting it back together.
// See PROTOCOL.md, "Long messages".
//
// One message holds about 28 KB of text. Longer text goes out as up to 16
// parts, each a normal message tagged ["part", id, index, total]; the
// receiver buffers them and delivers one message when all have arrived.

import (
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"unicode/utf8"
)

const (
	partBytes     = 24000  // JSON-escaped size of one part's text
	maxParts      = 16     //
	maxMessage    = 262144 // UTF-8 size of a whole message
	maxUnfinished = 4      // per sender
	unfinishedTTL = 3600   // seconds after the first part arrived
)

func escapedSize(r rune) int {
	switch {
	case strings.ContainsRune("\"\\\b\f\n\r\t", r):
		return 2
	case r < 0x20 || strings.ContainsRune("<>&  ", r):
		return 6
	}
	return utf8.RuneLen(r)
}

// splitMessage returns the parts to send text in (one, if it fits), or an
// error if it's too long to send at all.
func splitMessage(text string) ([]string, error) {
	if len(text) > maxMessage {
		return nil, fmt.Errorf("message is %d bytes; the limit is %d. Shorten it or send it in several messages", len(text), maxMessage)
	}
	var parts []string
	var cur strings.Builder
	size := 0
	for _, r := range text {
		n := escapedSize(r)
		if cur.Len() > 0 && size+n > partBytes {
			parts = append(parts, cur.String())
			cur.Reset()
			size = 0
		}
		cur.WriteRune(r)
		size += n
	}
	parts = append(parts, cur.String())
	if len(parts) > maxParts {
		return nil, fmt.Errorf("message needs %d parts; the limit is %d. Shorten it", len(parts), maxParts)
	}
	return parts, nil
}

func newPartID() string {
	b := make([]byte, 16)
	rand.Read(b)
	return hex.EncodeToString(b)
}

type partInfo struct {
	id           string
	index, total int
}

var partIDRE = regexp.MustCompile(`^[0-9a-f]{1,64}$`)

// parsePartTag reads a ["part", id, index, total] tag.
func parsePartTag(t []string) (partInfo, bool) {
	if len(t) < 4 || !partIDRE.MatchString(t[1]) {
		return partInfo{}, false
	}
	index, err1 := strconv.Atoi(t[2])
	total, err2 := strconv.Atoi(t[3])
	if err1 != nil || err2 != nil || total < 2 || total > maxParts || index < 1 || index > total {
		return partInfo{}, false
	}
	return partInfo{t[1], index, total}, true
}

// unfinished is a long message waiting for parts, in the "partials" document.
type unfinished struct {
	Sender string            `json:"sender"`
	Total  int               `json:"total"`
	Parts  map[string]string `json:"parts"`
	First  int64             `json:"first"`
	SentAt int64             `json:"sent_at,omitempty"`
	Ms     int64             `json:"ms,omitempty"`
}

// addPart buffers one part and returns the whole message once complete.
func addPart(buf map[string]*unfinished, m message, now int64) (message, bool) {
	key := m.sender + ":" + m.part.id
	u := buf[key]
	if u == nil {
		var mine []string
		for k, v := range buf {
			if v.Sender == m.sender {
				mine = append(mine, k)
			}
		}
		sort.Slice(mine, func(i, j int) bool { return buf[mine[i]].First < buf[mine[j]].First })
		for i := 0; i < len(mine)-maxUnfinished+1; i++ {
			delete(buf, mine[i])
		}
		u = &unfinished{Sender: m.sender, Total: m.part.total, Parts: map[string]string{}, First: now}
		buf[key] = u
	}
	idx := strconv.Itoa(m.part.index)
	if u.Total != m.part.total {
		return message{}, false
	}
	if _, dup := u.Parts[idx]; dup {
		return message{}, false
	}
	size := len(m.text)
	for _, t := range u.Parts {
		size += len(t)
	}
	if size > maxMessage {
		delete(buf, key)
		return message{}, false
	}
	u.Parts[idx] = m.text
	if m.part.index == 1 {
		u.SentAt, u.Ms = m.sentAt, m.ms
	}
	if len(u.Parts) < u.Total {
		return message{}, false
	}
	delete(buf, key)
	var b strings.Builder
	for i := 1; i <= u.Total; i++ {
		b.WriteString(u.Parts[strconv.Itoa(i)])
	}
	return message{sender: m.sender, text: b.String(), sentAt: u.SentAt, ms: u.Ms}, true
}

// expireParts removes messages unfinished for too long and returns them
// with markers where parts are missing.
func expireParts(buf map[string]*unfinished, now int64) []message {
	var out []message
	for k, u := range buf {
		if now-u.First <= unfinishedTTL {
			continue
		}
		delete(buf, k)
		var b strings.Builder
		for i := 1; i <= u.Total; i++ {
			if t, ok := u.Parts[strconv.Itoa(i)]; ok {
				b.WriteString(t)
			} else {
				fmt.Fprintf(&b, "[part %d of %d missing]", i, u.Total)
			}
		}
		sentAt := u.SentAt
		if sentAt == 0 {
			sentAt = u.First
		}
		out = append(out, message{sender: u.Sender, text: b.String(), sentAt: sentAt})
	}
	return out
}

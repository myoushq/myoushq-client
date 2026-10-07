package myous

import (
	"strings"
	"testing"
)

func TestSplitMessage(t *testing.T) {
	text := strings.Repeat("a", 30000) + strings.Repeat("😀", 2000) + strings.Repeat("\"<", 5000)
	parts, err := splitMessage(text)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Join(parts, "") != text {
		t.Fatal("parts don't join back to the text")
	}
	for i, p := range parts {
		size := 0
		for _, r := range p {
			size += escapedSize(r)
		}
		if size > partBytes {
			t.Errorf("part %d is %d escaped bytes", i+1, size)
		}
	}
	if one, _ := splitMessage("hello"); len(one) != 1 {
		t.Errorf("short text split into %d parts", len(one))
	}
	if _, err := splitMessage(strings.Repeat("a", maxMessage+1)); err == nil {
		t.Error("over-long message accepted")
	}
}

func TestReassembly(t *testing.T) {
	buf := map[string]*unfinished{}
	part := func(i int, text string) message {
		return message{sender: "s", text: text, sentAt: 100 + int64(i), part: &partInfo{"ab", i, 3}}
	}
	for _, m := range []message{part(3, "c"), part(1, "a"), part(1, "dup")} {
		if _, done := addPart(buf, m, 1000); done {
			t.Fatal("finished early")
		}
	}
	whole, done := addPart(buf, part(2, "b"), 1000)
	if !done || whole.text != "abc" || whole.sentAt != 101 || len(buf) != 0 {
		t.Fatalf("got %+v done=%v buf=%d", whole, done, len(buf))
	}

	addPart(buf, part(1, "x"), 1000)
	if got := expireParts(buf, 1000+unfinishedTTL+1); len(got) != 1 ||
		got[0].text != "x[part 2 of 3 missing][part 3 of 3 missing]" {
		t.Fatalf("expired: %+v", got)
	}
	for i := 0; i < maxUnfinished+2; i++ {
		addPart(buf, message{sender: "s", text: "t", part: &partInfo{string(rune('a' + i)), 1, 2}}, int64(i))
	}
	if len(buf) != maxUnfinished {
		t.Fatalf("holding %d unfinished messages from one sender", len(buf))
	}
}

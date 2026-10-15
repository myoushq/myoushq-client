package myous

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"sort"
	"strconv"
	"strings"
	"unicode"
	"unicode/utf8"
)

// The worker reads and writes JSON the way the Python worker does, so the
// two are interchangeable for the agents and apps that read what they
// write: Python's json.loads (which also takes NaN and Infinity) for
// requests, and json.dumps's defaults (non-ASCII as \u escapes, ", " and
// ": " between items) for replies, the log and the request records.

// pyKV is one member of a pyObj.
type pyKV struct {
	K string
	V any
}

// pyObj is a JSON object that keeps its members in order, as a Python dict
// does.
type pyObj []pyKV

// MarshalJSON lets a pyObj go through encoding/json (Storage.Put).
func (o pyObj) MarshalJSON() ([]byte, error) { return []byte(pyDumps(o, false)), nil }

// get returns a member's value.
func (o pyObj) get(k string) (any, bool) {
	for _, kv := range o {
		if kv.K == k {
			return kv.V, true
		}
	}
	return nil, false
}

// with returns o with k set to v (replaced in place if present, else
// added at the end), as dict(o, k=v) does.
func (o pyObj) with(k string, v any) pyObj {
	out := append(pyObj(nil), o...)
	for i := range out {
		if out[i].K == k {
			out[i].V = v
			return out
		}
	}
	return append(out, pyKV{k, v})
}

// pyDumps is json.dumps(v) (sortKeys: json.dumps(v, sort_keys=True)) for
// the values the worker writes.
func pyDumps(v any, sortKeys bool) string {
	var b strings.Builder
	pyEncode(&b, v, sortKeys)
	return b.String()
}

func pyEncode(b *strings.Builder, v any, sortKeys bool) {
	switch v := v.(type) {
	case nil:
		b.WriteString("null")
	case bool:
		if v {
			b.WriteString("true")
		} else {
			b.WriteString("false")
		}
	case string:
		pyString(b, v)
	case int:
		b.WriteString(strconv.Itoa(v))
	case int64:
		b.WriteString(strconv.FormatInt(v, 10))
	case float64:
		b.WriteString(pyFloat(v))
	case json.Number:
		if f, ok := pyNumber(v); ok && (math.IsNaN(f) || math.IsInf(f, 0) && strings.ContainsAny(v.String(), ".eE")) {
			b.WriteString(pyFloat(f)) // NaN or ±Infinity, as Python writes what pyLoads read
		} else {
			b.WriteString(v.String())
		}
	case pyObj:
		members := v
		if sortKeys {
			members = append(pyObj(nil), v...)
			sort.SliceStable(members, func(i, j int) bool { return members[i].K < members[j].K })
		}
		b.WriteByte('{')
		for i, kv := range members {
			if i > 0 {
				b.WriteString(", ")
			}
			pyString(b, kv.K)
			b.WriteString(": ")
			pyEncode(b, kv.V, sortKeys)
		}
		b.WriteByte('}')
	case map[string]any:
		o := make(pyObj, 0, len(v))
		for k, x := range v {
			o = append(o, pyKV{k, x})
		}
		sort.Slice(o, func(i, j int) bool { return o[i].K < o[j].K }) // a map has no order of its own
		pyEncode(b, o, sortKeys)
	case []any:
		b.WriteByte('[')
		for i, x := range v {
			if i > 0 {
				b.WriteString(", ")
			}
			pyEncode(b, x, sortKeys)
		}
		b.WriteByte(']')
	case []string:
		b.WriteByte('[')
		for i, x := range v {
			if i > 0 {
				b.WriteString(", ")
			}
			pyString(b, x)
		}
		b.WriteByte(']')
	default:
		raw, err := json.Marshal(v)
		if err != nil {
			b.WriteString("null")
			return
		}
		b.Write(raw)
	}
}

// pyString writes s as json.dumps does with ensure_ascii: everything
// outside printable ASCII as \u escapes (surrogate pairs above U+FFFF).
func pyString(b *strings.Builder, s string) {
	b.WriteByte('"')
	for _, r := range s { // invalid UTF-8 comes out as U+FFFD
		switch r {
		case '"':
			b.WriteString(`\"`)
		case '\\':
			b.WriteString(`\\`)
		case '\n':
			b.WriteString(`\n`)
		case '\r':
			b.WriteString(`\r`)
		case '\t':
			b.WriteString(`\t`)
		case '\b':
			b.WriteString(`\b`)
		case '\f':
			b.WriteString(`\f`)
		default:
			switch {
			case r >= 0x20 && r <= 0x7e:
				b.WriteRune(r)
			case r > 0xffff:
				r -= 0x10000
				fmt.Fprintf(b, `\u%04x\u%04x`, 0xd800+(r>>10), 0xdc00+(r&0x3ff))
			default:
				fmt.Fprintf(b, `\u%04x`, r)
			}
		}
	}
	b.WriteByte('"')
}

// pyFloat is Python's repr of a float, as json.dumps writes it.
func pyFloat(f float64) string {
	switch {
	case math.IsNaN(f):
		return "NaN"
	case math.IsInf(f, 1):
		return "Infinity"
	case math.IsInf(f, -1):
		return "-Infinity"
	}
	if f == 0 {
		if math.Signbit(f) {
			return "-0.0"
		}
		return "0.0"
	}
	exp := math.Floor(math.Log10(math.Abs(f)))
	if e := strconv.FormatFloat(f, 'e', -1, 64); strings.Contains(e, "e") {
		// The exponent of the shortest form, not log10's (which can be off by one).
		if n, err := strconv.Atoi(e[strings.Index(e, "e")+1:]); err == nil {
			exp = float64(n)
		}
	}
	if exp < -4 || exp >= 16 {
		return strconv.FormatFloat(f, 'e', -1, 64)
	}
	s := strconv.FormatFloat(f, 'f', -1, 64)
	if !strings.Contains(s, ".") {
		s += ".0"
	}
	return s
}

// pyLoads is json.loads for a request: numbers stay json.Number, and
// Python's NaN, Infinity and -Infinity are taken as the numbers they are
// there (pyNumber reads them as NaN and ±Inf): a ttl of NaN is refused,
// where a null ttl is no ttl.
func pyLoads(text string) (any, error) {
	dec := json.NewDecoder(strings.NewReader(nonFiniteNumbers(text)))
	dec.UseNumber()
	var v any
	if err := dec.Decode(&v); err != nil {
		return nil, err
	}
	if _, err := dec.Token(); !errors.Is(err, io.EOF) {
		return nil, errors.New("extra data after the JSON value")
	}
	return v, nil
}

// nanNumber is NaN in what pyLoads decodes. JSON has no literal for it;
// this one (a zero, written as no client writes it) stands in.
const nanNumber = json.Number("-0e-999999999")

// nonFiniteNumbers replaces Python's NaN, Infinity and -Infinity outside
// strings with JSON numbers that pyNumber reads as the same: nanNumber,
// 1e999 and -1e999 (which Python reads as ±inf too).
func nonFiniteNumbers(text string) string {
	if !strings.Contains(text, "NaN") && !strings.Contains(text, "Infinity") {
		return text
	}
	var b bytes.Buffer
	inString, escaped := false, false
	for i := 0; i < len(text); i++ {
		c := text[i]
		if inString {
			switch {
			case escaped:
				escaped = false
			case c == '\\':
				escaped = true
			case c == '"':
				inString = false
			}
			b.WriteByte(c)
			continue
		}
		if c == '"' {
			inString = true
		}
		replaced := false
		for _, tok := range [][2]string{{"-Infinity", "-1e999"}, {"Infinity", "1e999"}, {"NaN", string(nanNumber)}} {
			if strings.HasPrefix(text[i:], tok[0]) {
				b.WriteString(tok[1])
				i += len(tok[0]) - 1
				replaced = true
				break
			}
		}
		if !replaced {
			b.WriteByte(c)
		}
	}
	return b.String()
}

// pyNumber is a JSON number as Python would hold it: ok is false for
// anything that isn't a number (bools included). Infinite values come back
// as ±Inf, NaN as NaN.
func pyNumber(v any) (float64, bool) {
	n, ok := v.(json.Number)
	if !ok {
		return 0, false
	}
	if n == nanNumber {
		return math.NaN(), true
	}
	f, err := strconv.ParseFloat(n.String(), 64)
	if err != nil && !math.IsInf(f, 0) {
		return 0, false
	}
	return f, true
}

// pyIsSpace is Python's str.isspace for one character: Go's
// unicode.IsSpace plus the ASCII separators 0x1c-0x1f.
func pyIsSpace(r rune) bool { return unicode.IsSpace(r) || (r >= 0x1c && r <= 0x1f) }

// pyStrip is Python's str.strip().
func pyStrip(s string) string { return strings.TrimFunc(s, pyIsSpace) }

// pySplitLines is Python's str.splitlines() (without the trailing empty
// part Python drops; callers skip empty lines anyway).
func pySplitLines(s string) []string {
	return strings.FieldsFunc(strings.ReplaceAll(s, "\r\n", "\n"), func(r rune) bool {
		switch r {
		case '\n', '\r', '\v', '\f', 0x1c, 0x1d, 0x1e, 0x85, 0x2028, 0x2029:
			return true
		}
		return false
	})
}

// pyDecode is bytes.decode("utf-8", "replace"): each maximal invalid
// sequence becomes one U+FFFD, as Python does (Go would replace each
// byte), so output cut in the middle of a character reads the same.
func pyDecode(data []byte) string {
	if utf8.Valid(data) {
		return string(data)
	}
	var b strings.Builder
	for i := 0; i < len(data); {
		c := data[i]
		if c < 0x80 {
			b.WriteByte(c)
			i++
			continue
		}
		need, lo, hi := 0, byte(0x80), byte(0xbf)
		switch {
		case c >= 0xc2 && c <= 0xdf:
			need = 1
		case c == 0xe0:
			need, lo = 2, 0xa0
		case c == 0xed:
			need, hi = 2, 0x9f
		case c >= 0xe1 && c <= 0xef:
			need = 2
		case c == 0xf0:
			need, lo = 3, 0x90
		case c == 0xf4:
			need, hi = 3, 0x8f
		case c >= 0xf1 && c <= 0xf3:
			need = 3
		default:
			b.WriteRune(utf8.RuneError)
			i++
			continue
		}
		j, ok := i+1, true
		for k := 0; k < need; k++ {
			l, h := byte(0x80), byte(0xbf)
			if k == 0 {
				l, h = lo, hi
			}
			if j >= len(data) || data[j] < l || data[j] > h {
				ok = false
				break
			}
			j++
		}
		if ok {
			b.Write(data[i:j])
		} else {
			b.WriteRune(utf8.RuneError)
		}
		i = j
	}
	return b.String()
}

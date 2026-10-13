package myous

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"time"
	"unicode"
)

// The worker's review (protocol.md 7.2). With a review command (`myous
// worker --review CMD`) the worker runs it, as the Python worker does: the
// request as JSON on stdin, exit 0 allows, the first line of stderr is the
// reason. Without one, the worker applies the rules of worker/review.py
// itself (that file's docstring is the contract): the pause file refuses
// everything, then the level in <home>/review.json decides (trust,
// changes, all), and asking the owner goes through <home>/approvals.

// reviewRules is review.py's rule set, in one table: READ_ONLY,
// UNSAFE_TOKENS, READ_ONLY_POWERSHELL and UNSAFE_POWERSHELL. A test checks
// it against review.py, so the two can't drift apart.
var reviewRules = struct {
	ReadOnly, Unsafe, ReadOnlyPowerShell, UnsafePowerShell []string
}{
	// Commands that only read, for the "changes" level. A command line is
	// read-only when every segment (split on |, &&, ||, ;) starts with one
	// of these and nothing redirects output, substitutes commands or runs
	// code inside one (find -exec, awk '{...}').
	ReadOnly: []string{
		"ls", "cat", "head", "tail", "less", "more", "grep", "rg", "find", "wc", "stat", "file", "pwd",
		"du", "df", "uname", "date", "whoami", "id", "echo", "printf", "sort", "uniq", "cut", "tr",
		"awk", "diff", "tree", "which", "type", "true", "test", "basename", "dirname", "realpath",
		"readlink", "md5sum", "sha256sum", "shasum", "hostname", "nproc", "free", "uptime",
	},
	Unsafe: []string{">", "<", "$(", "`", "sed -i", "tee ", "{", "-exec", "-ok", "-delete"},
	// On Windows the worker's shell is PowerShell: its read-only commands,
	// and what runs code inside an otherwise harmless one ((...), & and
	// script blocks).
	ReadOnlyPowerShell: []string{
		"get-childitem", "gci", "dir", "get-content", "gc", "type", "get-item", "gi", "get-itemproperty",
		"get-location", "gl", "select-string", "sls", "measure-object", "measure", "get-date", "get-process",
		"gps", "get-command", "gcm", "test-path", "resolve-path", "get-filehash", "get-psdrive", "get-volume",
		"get-service", "write-output", "write-host", "format-table", "ft", "format-list", "fl", "out-string",
		"select-object", "select", "sort-object", "where.exe", "findstr", "ipconfig", "systeminfo",
	},
	UnsafePowerShell: []string{"(", "&", "@", "--%", "Invoke-", "iex"},
}

// Review levels (review.json's "level").
var reviewLevels = []string{"trust", "changes", "all"}

const (
	defaultApprovalWait = 120 // seconds the built-in review waits for the owner's answer
	pausedByOwner       = "the worker is paused by its owner"
)

// readOnly is review.py's read_only: whether a command line only reads,
// for the "changes" level. windows uses PowerShell's lists too.
func readOnly(cmd string, windows bool) bool {
	for _, tok := range reviewRules.Unsafe {
		if strings.Contains(cmd, tok) {
			return false
		}
	}
	if windows {
		lower := strings.ToLower(cmd)
		for _, tok := range reviewRules.UnsafePowerShell {
			if strings.Contains(lower, strings.ToLower(tok)) {
				return false
			}
		}
	}
	for _, sep := range []string{"||", "&&", "|", ";"} {
		cmd = strings.ReplaceAll(cmd, sep, "\n")
	}
	for _, segment := range pySplitLines(cmd) {
		words, err := shlexSplit(segment, !windows)
		if err != nil {
			return false
		}
		if len(words) == 0 {
			continue
		}
		if windows {
			w := strings.ToLower(words[0])
			if !slices.Contains(reviewRules.ReadOnly, w) && !slices.Contains(reviewRules.ReadOnlyPowerShell, w) {
				return false
			}
		} else if !slices.Contains(reviewRules.ReadOnly, words[0]) {
			return false
		}
	}
	return true
}

// builtinReview is review.py, in the worker.
type builtinReview struct {
	home      string        // the worker's data directory: review.json, approvals/
	pauseFile string        // refuse everything while it exists
	wait      time.Duration // how long to wait for the owner's answer
	limit     time.Duration // the review hook's time limit (the worker's REVIEW_TIMEOUT)
	windows   bool          // PowerShell's read-only lists too
}

// level is review.json's level, or "trust".
func (r *builtinReview) level() string {
	raw, err := os.ReadFile(filepath.Join(r.home, "review.json"))
	if err != nil {
		return "trust"
	}
	var doc map[string]any
	if json.Unmarshal(raw, &doc) != nil {
		return "trust"
	}
	if lv, _ := doc["level"].(string); slices.Contains(reviewLevels, lv) {
		return lv
	}
	return "trust"
}

// decide is review.py's decide: "" to allow, else why not.
func (r *builtinReview) decide(req pyObj) string {
	if r.pauseFile != "" && exists(r.pauseFile) {
		return pausedByOwner
	}
	if r.home == "" {
		return "" // nowhere to read a level from or ask: trust
	}
	lv := r.level()
	if lv == "trust" {
		return ""
	}
	op, _ := req.get("op")
	if lv == "changes" {
		cmd, _ := req.get("cmd")
		s, _ := cmd.(string)
		if op == "get" || (op == "exec" && readOnly(s, r.windows)) {
			return ""
		}
	}
	return r.ask(req)
}

// ask writes the question to approvals/<id>.json and waits for the
// owner's approvals/<id>.answer ("allow" or "refuse").
func (r *builtinReview) ask(req pyObj) string {
	dir := filepath.Join(r.home, "approvals")
	if err := os.MkdirAll(dir, 0o777); err != nil {
		return err.Error()
	}
	id, _ := req.get("id")
	sid, _ := id.(string)
	name := safeName(sid)
	if name == "" {
		name = strconv.FormatInt(time.Now().Unix(), 10)
	}
	question := filepath.Join(dir, name+".json")
	answer := filepath.Join(dir, name+".answer")
	q := req.with("asked_at", time.Now().Unix()).with("wait", r.wait.Seconds())
	if err := os.WriteFile(question+".tmp", []byte(pyDumps(q, false)), 0o666); err != nil {
		return err.Error()
	}
	if err := os.Rename(question+".tmp", question); err != nil {
		return err.Error()
	}
	defer os.Remove(question)
	// The hook runs inside the worker's time limit for it: an answer that
	// would come later is, as with a hook program, the hook not answering.
	wait, why := r.wait, "the owner didn't answer in time"
	if r.limit > 0 && wait > r.limit {
		wait, why = r.limit, "the review hook didn't answer"
	}
	deadline := time.Now().Add(wait)
	for time.Now().Before(deadline) {
		raw, err := os.ReadFile(answer)
		if errors.Is(err, os.ErrNotExist) {
			time.Sleep(500 * time.Millisecond)
			continue
		}
		if err != nil {
			return err.Error()
		}
		os.Remove(answer)
		if strings.ToLower(pyStrip(string(raw))) == "allow" {
			return ""
		}
		return "refused by the owner"
	}
	return why
}

// approvalWait is $MYOUS_APPROVAL_WAIT (seconds), default 120.
func approvalWait() time.Duration {
	if v := os.Getenv("MYOUS_APPROVAL_WAIT"); v != "" {
		if f, err := strconv.ParseFloat(strings.TrimSpace(v), 64); err == nil && f >= 0 {
			return time.Duration(f * float64(time.Second))
		}
	}
	return defaultApprovalWait * time.Second
}

// runReviewHook runs a review command with the request on stdin, in dir:
// "" to allow, else the first line of its stderr.
func runReviewHook(cmd, dir string, req pyObj, limit time.Duration) string {
	ctx, cancel := context.WithTimeout(context.Background(), limit)
	defer cancel()
	c := hookCommand(ctx, cmd)
	c.Dir = dir
	// ASCII, as the Python worker sends it: a hook reading stdin in the
	// system's code page (Windows) still gets the right characters.
	c.Stdin = strings.NewReader(pyDumps(req, false))
	var stderr strings.Builder
	c.Stderr = &stderr
	c.WaitDelay = 5 * time.Second
	err := c.Run()
	if ctx.Err() != nil {
		return "the review hook didn't answer"
	}
	var exit *exec.ExitError
	if err != nil && !errors.As(err, &exit) {
		return fmt.Sprintf("the review hook could not run (%v)", err)
	}
	if err == nil {
		return ""
	}
	if lines := pySplitLines(pyStrip(stderr.String())); len(lines) > 0 {
		return lines[0]
	}
	return "refused by the review hook"
}

// safeName is an id as a file name: letters, digits, - and _, at most 64
// characters (review.py's and the worker's rule).
func safeName(id string) string {
	var b strings.Builder
	n := 0
	for _, r := range id {
		if n == 64 {
			break
		}
		if unicode.IsLetter(r) || unicode.IsNumber(r) || r == '-' || r == '_' {
			b.WriteRune(r)
			n++
		}
	}
	return b.String()
}

func exists(path string) bool {
	_, err := os.Stat(path)
	return err == nil
}

// --- Python's shlex ------------------------------------------------------

// shlexSplit is Python's shlex.split(s, posix=posix): words split on
// whitespace, with quotes (kept in the words when not posix) and, when
// posix, backslash escapes. It fails where Python raises ValueError.
func shlexSplit(s string, posix bool) ([]string, error) {
	const eof = -1
	rs := []rune(s)
	pos := 0
	state := rune(' ')
	isSpace := func(r rune) bool { return r == ' ' || r == '\t' || r == '\r' || r == '\n' }
	isQuote := func(r rune) bool { return r == '\'' || r == '"' }
	var words []string
	for state != eof {
		var tok strings.Builder
		quoted := false
		escaped := rune(' ')
	read:
		for {
			atEnd := pos >= len(rs)
			var c rune
			if !atEnd {
				c = rs[pos]
				pos++
			}
			switch {
			case state == ' ':
				switch {
				case atEnd:
					state = eof
					break read
				case isSpace(c):
					if tok.Len() > 0 || (posix && quoted) {
						break read
					}
				case posix && c == '\\':
					escaped, state = 'a', '\\'
				case isQuote(c):
					if !posix {
						tok.WriteRune(c)
					}
					state = c
				default:
					tok.WriteRune(c)
					state = 'a'
				}
			case isQuote(state):
				quoted = true
				switch {
				case atEnd:
					return nil, errors.New("No closing quotation")
				case c == state:
					if !posix {
						tok.WriteRune(c)
						state = ' '
						break read
					}
					state = 'a'
				case posix && c == '\\' && state == '"':
					escaped, state = state, '\\'
				default:
					tok.WriteRune(c)
				}
			case state == '\\':
				if atEnd {
					return nil, errors.New("No escaped character")
				}
				// In quotes, only the quote itself or the backslash can be escaped.
				if isQuote(escaped) && c != state && c != escaped {
					tok.WriteRune(state)
				}
				tok.WriteRune(c)
				state = escaped
			default: // 'a', in a word
				switch {
				case atEnd:
					state = eof
					break read
				case isSpace(c):
					state = ' '
					if tok.Len() > 0 || (posix && quoted) {
						break read
					}
				case posix && isQuote(c):
					state = c
				case posix && c == '\\':
					escaped, state = 'a', '\\'
				default:
					tok.WriteRune(c)
				}
			}
		}
		if tok.Len() == 0 && !(posix && quoted) {
			break // the end
		}
		words = append(words, tok.String())
	}
	return words, nil
}

var shellUnsafe = regexp.MustCompile(`[^\w@%+=:,./-]`)

// shlexQuote is Python's shlex.quote.
func shlexQuote(s string) string {
	if s == "" {
		return "''"
	}
	if !shellUnsafe.MatchString(s) {
		return s
	}
	return "'" + strings.ReplaceAll(s, "'", `'"'"'`) + "'"
}

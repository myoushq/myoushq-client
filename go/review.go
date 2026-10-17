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
)

// The worker's review (protocol.md 7.2). With a review command (`myous
// worker --review CMD`) the worker runs it, as the Python worker does: the
// request as JSON on stdin, exit 0 allows, the first line of stderr is the
// reason. Without one, the worker applies the rules of worker/review.py
// itself (that file's docstring is the contract): the pause file refuses
// everything, then the level in <home>/review.json decides (trust,
// changes, all), and asking the owner goes through <home>/approvals.

// reviewRules is review.py's rule set, in one table: READ_ONLY,
// UNSAFE_TOKENS, READ_ONLY_POWERSHELL, UNSAFE_POWERSHELL, WINDOWS_PROGRAMS,
// SAFE_FLAGS, OPERANDS, AWK_UNSAFE and EXPANSION. A test checks it against
// review.py, so the two can't drift apart.
var reviewRules = struct {
	ReadOnly, Unsafe, ReadOnlyPowerShell, UnsafePowerShell, WindowsPrograms, AwkUnsafe []string
	SafeFlags                                                                          map[string][3]string
	Operands                                                                           map[string]string
	Expansion                                                                          string
}{
	// Commands that only read, for the "changes" level. A command line is
	// read-only when every segment (split on |, &&, ||, ;, &) starts with
	// one of these, nothing redirects output, substitutes commands, expands
	// a variable or runs code inside one (find -exec, awk '{...}'), every
	// flag is one known safe for the command (SafeFlags), and no operand
	// points outside the worker's work directory (escapesWork).
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
	// The Windows programs among those: their switches aren't PowerShell
	// parameters, so they take SafeFlags's rule (no entry: no flags), and a
	// /switch is checked like a path (so it asks).
	WindowsPrograms: []string{"where.exe", "findstr", "ipconfig", "systeminfo"},
	// At the "changes" level a read-only command runs unasked only when
	// every flag it carries is one known safe for it: one that only reads,
	// or formats what it prints. A flag that names a file to write, a
	// program to run or a command string, or that changes the system, is
	// never here, and a flag that isn't here asks. Per command: the safe
	// flags; those of them that take a value (attached, -n20 -d:
	// --lines=20, or the next word); and those whose value is a file, which
	// must stay in the work directory like an operand. Values are not
	// checked as paths, so a flag is listed as taking one only where every
	// version of the command (GNU, BSD, macOS) takes one. Short flags
	// combine; a word that is itself a listed flag is that flag (find
	// -name, test -nt). A command without an entry allows no flags.
	// review.py's SAFE_FLAGS says more.
	SafeFlags: map[string][3]string{
		"awk":      {"-F -v", "-F -v", ""},
		"basename": {"-a -s -z --multiple --suffix --zero", "-s --suffix", ""},
		"cat": {
			"-A -E -T -b -e -n -s -t -u -v --number --number-nonblank --show-all --show-ends --show-nonprinting " +
				"--show-tabs --squeeze-blank",
			"",
			"",
		},
		"cut": {
			"-b -c -d -f -n -s -z --bytes --characters --complement --delimiter --fields --only-delimited " +
				"--output-delimiter --zero-terminated",
			"-b -c -d -f --bytes --characters --delimiter --fields --output-delimiter",
			"",
		},
		"date": {
			"-I -R -j -r -u --iso-8601 --reference --rfc-2822 --rfc-3339 --rfc-email --universal --utc",
			"-r --reference --rfc-3339",
			"-r --reference",
		},
		"df": {
			"-H -P -T -a -h -i -k -l --all --human-readable --inodes --local --portability --print-type --si",
			"",
			"",
		},
		"diff": {
			"-B -N -U -a -b -c -i -q -r -s -t -u -w -x -y --brief --color --context --exclude --expand-tabs " +
				"--from-file --ignore-all-space --ignore-blank-lines --ignore-case --ignore-space-change --new-file " +
				"--recursive --report-identical-files --side-by-side --strip-trailing-cr --text --to-file --unified",
			"-U -x --exclude --from-file --to-file",
			"--from-file --to-file",
		},
		"dirname": {"-z --zero", "", ""},
		"du": {
			"-a -c -d -h -k -m -s -x --all --apparent-size --exclude --human-readable --max-depth " +
				"--one-file-system --summarize --total",
			"-d --exclude --max-depth",
			"",
		},
		"echo": {"-E -e -n", "", ""},
		"file": {
			"-I -L -N -b -h -i -k --brief --dereference --keep-going --mime --mime-encoding --mime-type " +
				"--no-dereference --no-pad",
			"",
			"",
		},
		"find": {
			"-a -amin -and -anewer -atime -cmin -cnewer -ctime -depth -empty -executable -false -gid -group " +
				"-iname -inum -ipath -iregex -iwholename -links -ls -maxdepth -mindepth -mmin -mount -mtime -name " +
				"-newer -nogroup -not -nouser -o -or -path -perm -print -print0 -printf -prune -quit -readable -regex " +
				"-regextype -samefile -size -true -type -uid -user -wholename -writable -xdev -xtype",
			"-amin -anewer -atime -cmin -cnewer -ctime -gid -group -iname -inum -ipath -iregex -iwholename -links " +
				"-maxdepth -mindepth -mmin -mtime -name -newer -path -perm -printf -regex -regextype -samefile -size " +
				"-type -uid -user -wholename -xtype",
			"-anewer -cnewer -newer -samefile",
		},
		"free": {"-b -g -h -k -m -t -w --bytes --giga --human --kilo --mega --total --wide", "", ""},
		"grep": {
			"-0 -1 -2 -3 -4 -5 -6 -7 -8 -9 -A -B -C -E -F -G -H -I -L -R -a -b -c -e -f -h -i -l -m -n -o -q -r " +
				"-s -v -w -x --after-context --basic-regexp --before-context --byte-offset --color --colour --context " +
				"--count --dereference-recursive --exclude --exclude-dir --extended-regexp --file " +
				"--files-with-matches --files-without-match --fixed-strings --ignore-case --include --invert-match " +
				"--line-number --line-regexp --max-count --no-filename --no-messages --only-matching --quiet " +
				"--recursive --regexp --silent --text --with-filename --word-regexp",
			"-e -f --exclude --exclude-dir --file --include --regexp",
			"-f --file",
		},
		"head": {
			"-0 -1 -2 -3 -4 -5 -6 -7 -8 -9 -c -n -q -v --bytes --lines --quiet --silent --verbose",
			"-c -n --bytes --lines",
			"",
		},
		"id": {"-G -g -n -p -r -u --group --groups --name --real --user", "", ""},
		"less": {
			"-E -F -I -J -M -N -Q -R -S -W -X -e -f -g -i -m -n -p -q -r -s -w -x --IGNORE-CASE --LINE-NUMBERS " +
				"--RAW-CONTROL-CHARS --chop-long-lines --ignore-case --no-init --pattern --quit-at-eof " +
				"--quit-if-one-screen --squeeze-blank-lines --tabs",
			"-p -x --pattern --tabs",
			"",
		},
		"ls": {
			"-1 -A -C -F -G -L -R -S -a -c -d -h -i -k -l -m -n -o -p -r -s -t -u -x --all --almost-all " +
				"--classify --color --dereference --directory --full-time --group-directories-first --human-readable " +
				"--inode --numeric-uid-gid --recursive --reverse --size",
			"",
			"",
		},
		"md5sum": {
			"-b -c -t -w -z --binary --check --ignore-missing --quiet --status --strict --tag --text --warn " +
				"--zero",
			"",
			"",
		},
		"more":  {"-c -d -f -l -p -s -u", "", ""},
		"nproc": {"--all", "", ""},
		"pwd":   {"-L -P", "", ""},
		"readlink": {
			"-e -f -m -n -q -s -v -z --canonicalize --canonicalize-existing --canonicalize-missing --no-newline " +
				"--quiet --silent --verbose --zero",
			"",
			"",
		},
		"realpath": {
			"-L -P -e -m -q -s -z --canonicalize-existing --canonicalize-missing --logical --no-symlinks " +
				"--physical --quiet --strip --zero",
			"",
			"",
		},
		"rg": {
			"-0 -A -B -C -F -H -I -M -N -S -T -U -a -c -d -e -f -g -i -j -l -m -n -o -p -q -r -s -t -u -v -w -x " +
				"--after-context --before-context --case-sensitive --color --column --context --count --count-matches " +
				"--file --files --files-with-matches --files-without-match --fixed-strings --glob --heading --hidden " +
				"--iglob --ignore-case --invert-match --json --line-number --line-regexp --max-columns --max-count " +
				"--max-depth --no-filename --no-heading --no-ignore --no-line-number --null --only-matching --pretty " +
				"--quiet --regexp --replace --smart-case --sort --sortr --stats --threads --trim --type --type-list " +
				"--type-not --vimgrep --with-filename --word-regexp",
			"-A -B -C -M -T -d -e -f -g -j -m -r -t --after-context --before-context --color --context --file " +
				"--glob --iglob --max-columns --max-count --max-depth --regexp --replace --sort --sortr --threads " +
				"--type --type-not",
			"-f --file",
		},
		"sha256sum": {
			"-b -c -t -w -z --binary --check --ignore-missing --quiet --status --strict --tag --text --warn " +
				"--zero",
			"",
			"",
		},
		"shasum": {
			"-0 -a -b -c -t --algorithm --binary --check --ignore-missing --quiet --status --strict --tag --text " +
				"--warn",
			"-a --algorithm",
			"",
		},
		"sort": {
			"-C -M -V -b -c -d -f -g -h -i -k -m -n -r -s -t -u -z --check --dictionary-order --field-separator " +
				"--general-numeric-sort --human-numeric-sort --ignore-case --ignore-leading-blanks " +
				"--ignore-nonprinting --key --merge --month-sort --numeric-sort --reverse --stable --unique " +
				"--version-sort --zero-terminated",
			"-k -t --field-separator --key",
			"",
		},
		"stat": {"-L -c -f -l -t --dereference --file-system --format --printf --terse", "-c --format --printf", ""},
		"tail": {
			"-0 -1 -2 -3 -4 -5 -6 -7 -8 -9 -F -c -f -n -q -r -v --bytes --follow --lines --quiet --retry --silent " +
				"--verbose",
			"-c -n --bytes --lines",
			"",
		},
		"test": {
			"-G -L -N -O -S -a -b -c -d -e -ef -eq -f -g -ge -gt -h -k -le -lt -n -ne -nt -o -ot -p -r -s -t -u " +
				"-w -x -z",
			"",
			"",
		},
		"tr": {"-C -c -d -s -t --complement --delete --squeeze-repeats --truncate-set1", "", ""},
		"tree": {
			"-A -C -D -F -I -J -L -N -P -Q -S -U -X -a -c -d -f -g -h -i -n -p -r -s -t -u -v --dirsfirst --du " +
				"--ignore-case --matchdirs --noreport --prune",
			"-I -L -P",
			"",
		},
		"type": {"-P -a -p -t", "", ""},
		"uname": {
			"-a -i -m -n -o -p -r -s -v --all --hardware-platform --kernel-name --kernel-release --kernel-version " +
				"--machine --nodename --operating-system --processor",
			"",
			"",
		},
		"uniq": {
			"-D -c -d -f -i -s -u -w --all-repeated --check-chars --count --ignore-case --repeated --skip-chars " +
				"--skip-fields --unique",
			"-f -s -w --check-chars --skip-chars --skip-fields",
			"",
		},
		"uptime": {"-p -s --pretty --since", "", ""},
		"wc":     {"-L -c -l -m -w --bytes --chars --lines --max-line-length --words", "", ""},
		"which":  {"-a -s", "", ""},
	},
	// Operands that aren't only paths in the work directory: awk's first is
	// its program (AwkUnsafe asks), date's must be +FORMAT (any other sets
	// the clock), hostname NAME sets the name, uniq's second is a file it
	// writes, less and more run a +command.
	Operands:  map[string]string{"awk": "program", "date": "+format", "hostname": "none", "less": "no+", "more": "no+", "uniq": "one"},
	AwkUnsafe: []string{"system", "getline", "|", ">", "{", "@", "environ"}, // in the program, in lower case
	// $NAME, ${...}, $(...), $1, $? (and PowerShell's $env:X, $HOME): the
	// shell puts in a value the review can't see, a path or a secret; the
	// split words no longer say whether it was quoted, so any of these asks.
	Expansion: `\$[A-Za-z0-9_{(?!$#@*-]`,
}

var expansion = regexp.MustCompile(reviewRules.Expansion)

// Review levels (review.json's "level").
var reviewLevels = []string{"trust", "changes", "all"}

const (
	defaultApprovalWait = 120 // seconds the built-in review waits for the owner's answer
	pausedByOwner       = "the worker is paused by its owner"
)

// unquoted is review.py's _unquoted: a word as the command gets it.
// Non-POSIX shlex (Windows) keeps the quotes around a word, and one layer
// of them comes off.
func unquoted(word string, windows bool) string {
	if windows && len(word) >= 2 && word[0] == word[len(word)-1] && (word[0] == '\'' || word[0] == '"') {
		return word[1 : len(word)-1]
	}
	return word
}

// escapesWork is review.py's _escapes_work: whether an operand, or a file a
// flag names, could point outside the work directory (W3). The work
// directory is the command's cwd, so a plain relative path stays inside
// it; an absolute path, a ~ the shell expands to a home directory, a ".."
// component, or a pattern the shell can expand to one (".*" matches "..")
// can leave it. On Windows also any PowerShell drive (C:, env:, HKCU:: any
// colon), a path that starts with a backslash, and each part of a comma
// list (an array to PowerShell). Conservative: a word that merely looks
// like a path is treated as one, so the owner is asked rather than the
// command run unseen -- more prompts, never fewer.
func escapesWork(token string, windows bool) bool {
	t := unquoted(token, windows)
	if windows && strings.Contains(t, ",") {
		for _, part := range strings.Split(t, ",") {
			if escapesWork(part, windows) {
				return true
			}
		}
		return false
	}
	if t == "" {
		return false
	}
	if t[0] == '~' || t[0] == '/' || (windows && (t[0] == '\\' || strings.Contains(t, ":"))) {
		return true
	}
	sep := "/"
	if windows {
		sep = `\/`
	}
	for _, part := range strings.FieldsFunc(t, func(r rune) bool { return strings.ContainsRune(sep, r) }) {
		if part == ".." || (strings.HasPrefix(part, ".") && strings.ContainsAny(part, "*?[")) {
			return true
		}
	}
	return false
}

// flagsSafe is review.py's _flags_safe: whether a read-only command's flags
// are all known safe for it (SafeFlags), its operands are what Operands
// allows, and they and the files its flags name stay in the work directory
// (escapesWork). After "--" every word is an operand; a bare "-" is stdin.
func flagsSafe(words []string, windows bool) bool {
	name := words[0]
	if windows {
		name = strings.ToLower(name)
	}
	rule := reviewRules.SafeFlags[name]
	flags, valued, files := strings.Fields(rule[0]), strings.Fields(rule[1]), strings.Fields(rule[2])
	// value takes a flag's value: attached, or the next word (false when
	// there is none), and checks it if it is a file.
	var operands []string
	rest := words[1:]
	value := func(flag, attached string, eq bool) bool {
		if !eq && attached == "" && slices.Contains(valued, flag) {
			if len(rest) == 0 {
				return false
			}
			attached, rest = unquoted(rest[0], windows), rest[1:]
		}
		return !slices.Contains(files, flag) || !escapesWork(attached, windows)
	}
	for len(rest) > 0 {
		word := rest[0]
		rest = rest[1:]
		w := unquoted(word, windows)
		if w != word && (w == "-" || w == "--") {
			return false // quoted (Windows): PowerShell may join it to the next word
		}
		if w == "--" {
			for _, x := range rest {
				operands = append(operands, unquoted(x, windows))
			}
			break
		}
		if w == "-" || !strings.HasPrefix(w, "-") {
			operands = append(operands, w)
			continue
		}
		if strings.HasPrefix(w, "--") || slices.Contains(flags, w) {
			flag, attached, eq := w, "", false
			if strings.HasPrefix(w, "--") {
				flag, attached, eq = strings.Cut(w, "=")
			}
			if !slices.Contains(flags, flag) || !value(flag, attached, eq) {
				return false
			}
			continue
		}
		rs := []rune(w) // -la, -n20, -d:
		for j := 1; j < len(rs); j++ {
			flag := "-" + string(rs[j])
			if !slices.Contains(flags, flag) {
				return false
			}
			if slices.Contains(valued, flag) {
				if !value(flag, string(rs[j+1:]), false) {
					return false
				}
				break
			}
		}
	}
	switch reviewRules.Operands[name] {
	case "program":
		if len(operands) == 0 {
			return false
		}
		program := strings.ToLower(operands[0])
		for _, bad := range reviewRules.AwkUnsafe {
			if strings.Contains(program, bad) {
				return false
			}
		}
		operands = operands[1:]
	case "none":
		if len(operands) > 0 {
			return false
		}
	case "one":
		if len(operands) > 1 {
			return false
		}
	case "+format":
		for _, o := range operands {
			if !strings.HasPrefix(o, "+") {
				return false
			}
		}
	case "no+":
		for _, o := range operands {
			if strings.HasPrefix(o, "+") {
				return false
			}
		}
	}
	for _, o := range operands {
		if escapesWork(o, windows) {
			return false
		}
	}
	return true
}

// readOnly is review.py's read_only: whether a command line only reads
// within the work directory, for the "changes" level. windows uses
// PowerShell's lists too.
func readOnly(cmd string, windows bool) bool {
	for _, tok := range reviewRules.Unsafe {
		if strings.Contains(cmd, tok) {
			return false
		}
	}
	if expansion.MatchString(cmd) {
		return false
	}
	if windows {
		lower := strings.ToLower(cmd)
		for _, tok := range reviewRules.UnsafePowerShell {
			if strings.Contains(lower, strings.ToLower(tok)) {
				return false
			}
		}
	}
	// && before &: once && is gone, the remaining & are single backgrounders
	// (`echo & rm x` runs rm x; without & as a separator only `echo` was seen).
	for _, sep := range []string{"||", "&&", "|", ";", "&"} {
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
		verb := words[0]
		if windows {
			verb = strings.ToLower(verb)
		}
		if windows && slices.Contains(reviewRules.ReadOnlyPowerShell, verb) && !slices.Contains(reviewRules.WindowsPrograms, verb) {
			// PowerShell's own cmdlets and aliases: their parameters are
			// PowerShell's (-Recurse, -Path x, -Path:x); their paths must stay
			// in the work directory.
			for _, w := range words[1:] {
				if strings.HasPrefix(w, "-") {
					_, w, _ = strings.Cut(w, ":")
				}
				if escapesWork(w, windows) {
					return false
				}
			}
			continue
		}
		if !slices.Contains(reviewRules.ReadOnly, verb) && !(windows && slices.Contains(reviewRules.WindowsPrograms, verb)) {
			return false
		}
		if writesAFile(words) || !flagsSafe(words, windows) {
			return false
		}
	}
	return true
}

// writesAFile is whether an otherwise read-only command writes a file
// through one of its own flags: `sort -o/--output FILE` (also `-oFILE`),
// and GNU find's -fprint/-fprintf/-fprint0/-fls. Command-specific (a bare
// "-o" is harmless for grep), so not a global Unsafe token.
func writesAFile(words []string) bool {
	switch words[0] {
	case "sort":
		for _, a := range words[1:] {
			if a == "-o" || a == "--output" || strings.HasPrefix(a, "--output=") ||
				(strings.HasPrefix(a, "-o") && !strings.HasPrefix(a, "--") && len(a) > 2) {
				return true
			}
		}
	case "find":
		for _, a := range words[1:] {
			if a == "-fprint" || a == "-fprintf" || a == "-fprint0" || a == "-fls" {
				return true
			}
		}
	}
	return false
}

// builtinReview is review.py, in the worker.
type builtinReview struct {
	home      string        // the worker's data directory: review.json, approvals/
	pauseFile string        // refuse everything while it exists
	wait      time.Duration // how long to wait for the owner's answer
	limit     time.Duration // the review hook's time limit (the worker's REVIEW_TIMEOUT)
	windows   bool          // PowerShell's read-only lists too
}

// failClosed is the level for a review.json that is present but can't be
// read as a valid level (review.py's FAIL_CLOSED): ask about everything,
// rather than running everything unreviewed. W2 used a truncated
// review.json to drop the worker to "trust"; this makes a corrupt or
// unreadable setting file fail safe. An absent file is a worker that was
// never configured, and keeps the default (defaultLevel).
const failClosed = "all"

// note appends one line to worker.log, as the worker writes its own
// (review.py's note): for what the review can't say in a reason.
func (r *builtinReview) note(record pyObj) {
	f, err := os.OpenFile(filepath.Join(r.home, "worker.log"), os.O_WRONLY|os.O_CREATE|os.O_APPEND, 0o666)
	if err != nil {
		return
	}
	defer f.Close()
	f.WriteString(pyDumps(record.with("at", time.Now().Unix()), true) + "\n")
}

// defaultLevel is review.py's default_level: the level of a worker that has
// never been configured (no review.json), $MYOUS_DEFAULT_REVIEW, or "trust"
// when that is unset or empty. The container's entrypoint sets it to
// "changes", so a fresh container asks before anything that changes things,
// as direct mode (the desktop app) already does; a level picked in the app
// writes review.json, which then wins. A value that isn't a level fails
// closed like a corrupt review.json (an unreadable safety setting asks, it
// doesn't run everything), and says so in worker.log.
func (r *builtinReview) defaultLevel() string {
	lv := os.Getenv("MYOUS_DEFAULT_REVIEW")
	if lv == "" {
		return "trust"
	}
	if slices.Contains(reviewLevels, lv) {
		return lv
	}
	r.note(pyObj{{"op", "review"}, {"value", lv},
		{"error", "MYOUS_DEFAULT_REVIEW isn't trust, changes or all, so every request is asked about (all)"}})
	return failClosed
}

// level is review.py's level: review.json's level. Absent: the default
// (defaultLevel). Present but unreadable, not JSON, not an object or not a
// level: failClosed.
func (r *builtinReview) level() string {
	raw, err := os.ReadFile(filepath.Join(r.home, "review.json"))
	if err != nil {
		if os.IsNotExist(err) {
			return r.defaultLevel() // never configured
		}
		return failClosed // present but unreadable
	}
	var doc map[string]any
	if json.Unmarshal(raw, &doc) != nil {
		return failClosed
	}
	if lv, _ := doc["level"].(string); slices.Contains(reviewLevels, lv) {
		return lv
	}
	return failClosed
}

// decide is review.py's decide: "" to allow, else why not.
func (r *builtinReview) decide(ctx context.Context, req pyObj) string {
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
	return r.ask(ctx, req)
}

// ask writes the question to approvals/<id>.json and waits for the
// owner's approvals/<id>.answer ("allow" or "refuse"). The id names the
// files, so one that isn't a plain name (validID) is refused, as review.py
// does: the owner's app couldn't answer it. ctx ending (the worker
// stopping) ends the wait.
func (r *builtinReview) ask(ctx context.Context, req pyObj) string {
	id, _ := req.get("id")
	name, _ := id.(string)
	if !validID(name) {
		return badID
	}
	dir := filepath.Join(r.home, "approvals")
	if err := os.MkdirAll(dir, 0o777); err != nil {
		return err.Error()
	}
	question := filepath.Join(dir, name+".json")
	answer := filepath.Join(dir, name+".answer")
	// Only an answer written for this question counts. A leftover
	// <id>.answer (the owner clicked after the wait ran out, or a stop
	// killed the hook before it read the answer) must not approve a later
	// request that reuses the id: the agent chooses ids.
	os.Remove(answer)
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
			select {
			case <-ctx.Done():
				return stoppingWhy
			case <-time.After(500 * time.Millisecond):
			}
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
// "" to allow, else the first line of its stderr. ctx ending (the worker
// stopping) ends the hook.
func runReviewHook(stop context.Context, cmd, dir string, req pyObj, limit time.Duration) string {
	ctx, cancel := context.WithTimeout(stop, limit)
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
	if stop.Err() != nil {
		return stoppingWhy
	}
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

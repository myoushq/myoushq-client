package myous

// The built-in review: a port of python/tests/review_hook_test.py, and
// checks that its rules are review.py's (the table, and the answers for
// the same commands, when a Python is there to ask).

import (
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"testing"
	"time"
)

func TestReviewReadOnlyCommands(t *testing.T) {
	for _, ok := range []string{"ls -la", "cat a.txt | grep x | wc -l", "find . -name '*.py' && echo done", "pwd"} {
		if !readOnly(ok, false) {
			t.Errorf("%q should be read-only", ok)
		}
	}
	for _, bad := range []string{"rm -rf x", "cat a > b", "echo $(whoami)", "ls; python3 x.py", "sed -i s/a/b/ f", "cat `ls`", "curl http://x"} {
		if readOnly(bad, false) {
			t.Errorf("%q isn't read-only", bad)
		}
	}
}

func TestReviewCodeInsideAReadOnlyCommand(t *testing.T) {
	for _, bad := range []string{"find . -exec rm {} ;", "find . -delete", `awk 'BEGIN{system("rm x")}'`} {
		if readOnly(bad, false) {
			t.Errorf("%q isn't read-only", bad)
		}
	}
}

func TestReviewPowerShell(t *testing.T) {
	for _, ok := range []string{"Get-ChildItem -Recurse | Select-String foo", `gc C:\work\a.txt | Measure-Object -Line`, "dir; Get-Date"} {
		if !readOnly(ok, true) {
			t.Errorf("%q should be read-only", ok)
		}
	}
	for _, bad := range []string{"Remove-Item x", "gci | Remove-Item", "gc (Remove-Item x)", "& notepad", "gci | where { Remove-Item $_ }",
		"Get-Content a > b", "iex 'rm x'", "Invoke-WebRequest http://x"} {
		if readOnly(bad, true) {
			t.Errorf("%q isn't read-only", bad)
		}
	}
}

func reviewFor(t *testing.T) (*builtinReview, string) {
	home := t.TempDir()
	return &builtinReview{home: home, pauseFile: filepath.Join(home, "worker.paused"), wait: 3 * time.Second,
		limit: reviewTimeout}, home
}

func setLevel(t *testing.T, home, level string) {
	if err := os.WriteFile(filepath.Join(home, "review.json"), []byte(`{"level": "`+level+`"}`), 0o600); err != nil {
		t.Fatal(err)
	}
}

func req(op, id string, fields ...pyKV) pyObj {
	return append(pyObj{{"op", op}, {"id", id}, {"sender", "npub1x"}, {"alias", "muse"}}, fields...)
}

func TestReviewTrustAllowsAndPauseRefuses(t *testing.T) {
	r, home := reviewFor(t)
	if why := r.decide(context.Background(), req("exec", "a", pyKV{"cmd", "rm -rf x"})); why != "" {
		t.Fatal(why)
	}
	os.WriteFile(filepath.Join(home, "worker.paused"), nil, 0o600)
	if why := r.decide(context.Background(), req("exec", "a", pyKV{"cmd", "ls"})); why != "the worker is paused by its owner" {
		t.Fatal(why)
	}
}

func TestReviewChangesLevel(t *testing.T) {
	r, home := reviewFor(t)
	setLevel(t, home, "changes")
	if why := r.decide(context.Background(), req("exec", "a", pyKV{"cmd", "ls -la"})); why != "" {
		t.Fatal(why)
	}
	if why := r.decide(context.Background(), req("get", "b", pyKV{"path", "x"})); why != "" {
		t.Fatal(why)
	}
	// A change with nobody answering: refused after the wait, question removed.
	if why := r.decide(context.Background(), req("put", "c", pyKV{"path", "x"}, pyKV{"size", 1})); why != "the owner didn't answer in time" {
		t.Fatal(why)
	}
	if left, _ := os.ReadDir(filepath.Join(home, "approvals")); len(left) != 0 {
		t.Fatalf("left behind: %v", left)
	}
}

func TestReviewAllLevelAsksAndTakesTheAnswer(t *testing.T) {
	r, home := reviewFor(t)
	setLevel(t, home, "all")
	approvals := filepath.Join(home, "approvals")
	answer := func(verdict string, done chan<- map[string]any) {
		for i := 0; i < 40; i++ {
			qs, _ := filepath.Glob(filepath.Join(approvals, "*.json"))
			if len(qs) > 0 {
				raw, _ := os.ReadFile(qs[0])
				var q map[string]any
				json.Unmarshal(raw, &q)
				os.WriteFile(strings.TrimSuffix(qs[0], ".json")+".answer", []byte(verdict+"\n"), 0o600)
				done <- q
				return
			}
			time.Sleep(100 * time.Millisecond)
		}
		done <- nil
	}
	done := make(chan map[string]any, 1)
	go answer("allow", done)
	if why := r.decide(context.Background(), req("exec", "q1", pyKV{"cmd", "ls"})); why != "" {
		t.Fatal(why)
	}
	q := <-done
	if q["op"] != "exec" || q["cmd"] != "ls" || q["wait"] != 3.0 || q["asked_at"] == nil {
		t.Fatalf("question: %v", q)
	}
	go answer("Refuse", done)
	if why := r.decide(context.Background(), req("exec", "q2", pyKV{"cmd", "ls"})); why != "refused by the owner" {
		t.Fatal(why)
	}
	<-done
	// Ids with - and _ are asked about under their own name, which the apps answer.
	go answer("allow", done)
	if why := r.decide(context.Background(), req("exec", "q-2_b", pyKV{"cmd", "ls"})); why != "" {
		t.Fatal(why)
	}
	<-done
	// Ids the apps can't answer are refused, not asked about (as review.py).
	for _, id := range []string{"a.b", "caf\u00e9", strings.Repeat("x", 65), ""} {
		if why := r.decide(context.Background(), req("exec", id, pyKV{"cmd", "ls"})); why != badID {
			t.Fatalf("%q: %s", id, why)
		}
	}
	// Stopping the worker ends the wait.
	ctx, cancel := context.WithCancel(context.Background())
	time.AfterFunc(300*time.Millisecond, cancel)
	if why := r.decide(ctx, req("exec", "q-stop", pyKV{"cmd", "ls"})); why != stoppingWhy {
		t.Fatal(why)
	}
	if left, _ := os.ReadDir(approvals); len(left) != 0 {
		t.Fatalf("left behind: %v", left)
	}
	// A wait longer than the worker allows a review: the review's limit, as with a hook program.
	r.wait, r.limit = time.Hour, time.Second
	if why := r.decide(context.Background(), req("exec", "q3", pyKV{"cmd", "ls"})); why != "the review hook didn't answer" {
		t.Fatal(why)
	}
	// The question file is what review.py writes.
	r.wait, r.limit = 120*time.Second, reviewTimeout
	go func() {
		for !exists(filepath.Join(approvals, "q4.json")) {
			time.Sleep(20 * time.Millisecond)
		}
		raw, _ := os.ReadFile(filepath.Join(approvals, "q4.json"))
		done <- map[string]any{"raw": string(raw)}
		os.WriteFile(filepath.Join(approvals, "q4.answer"), []byte("allow"), 0o600)
	}()
	r.decide(context.Background(), req("exec", "q4", pyKV{"cmd", "ls é"}))
	raw := (<-done)["raw"].(string)
	prefix := `{"op": "exec", "id": "q4", "sender": "npub1x", "alias": "muse", "cmd": "ls \u00e9", "asked_at": `
	if !strings.HasPrefix(raw, prefix) || !strings.HasSuffix(raw, `, "wait": 120.0}`) {
		t.Fatalf("question file: %s", raw)
	}
}

// python3 for the comparisons with review.py, or "".
func python3(t *testing.T) string {
	for _, name := range []string{os.Getenv("MYOUS_PYTHON"), "python3", "python"} {
		if name == "" {
			continue
		}
		if p, err := exec.LookPath(name); err == nil {
			if out, err := exec.Command(p, "-c", "import sys; print(sys.version_info[0])").Output(); err == nil && strings.TrimSpace(string(out)) == "3" {
				return p
			}
		}
	}
	t.Skip("no Python 3 to compare with")
	return ""
}

func reviewPy(t *testing.T) string {
	path, err := filepath.Abs(filepath.Join("..", "worker", "review.py"))
	if err != nil || !exists(path) {
		t.Skip("worker/review.py isn't here")
	}
	return path
}

// askPython runs code with review.py loaded as `r` (and json, shlex) and
// decodes the JSON it prints.
func askPython(t *testing.T, code string, input any, out any) {
	t.Helper()
	py, hook := python3(t), reviewPy(t)
	prog := "import importlib.util, json, shlex, sys\n" +
		"spec = importlib.util.spec_from_file_location('review_hook', sys.argv[1])\n" +
		"r = importlib.util.module_from_spec(spec); spec.loader.exec_module(r)\n" +
		"data = json.load(sys.stdin)\n" + code
	c := exec.Command(py, "-c", prog, hook)
	in, _ := json.Marshal(input)
	c.Stdin = strings.NewReader(string(in))
	c.Env = append(os.Environ(), "MYOUS_HOME="+t.TempDir(), "PYTHONIOENCODING=utf-8") // the test sends raw UTF-8 (the worker sends ASCII)
	raw, err := c.Output()
	if err != nil {
		if ee, ok := err.(*exec.ExitError); ok {
			t.Fatalf("python: %v\n%s", err, ee.Stderr)
		}
		t.Fatal(err)
	}
	if err := json.Unmarshal(raw, out); err != nil {
		t.Fatalf("python said %s: %v", raw, err)
	}
}

func TestReviewRulesAreReviewPys(t *testing.T) {
	var lists map[string][]string
	askPython(t, "print(json.dumps({'ReadOnly': sorted(r.READ_ONLY), 'Unsafe': list(r.UNSAFE_TOKENS), "+
		"'ReadOnlyPowerShell': sorted(r.READ_ONLY_POWERSHELL), 'UnsafePowerShell': list(r.UNSAFE_POWERSHELL)}))", nil, &lists)
	sorted := func(s []string) []string { s = slices.Clone(s); slices.Sort(s); return s }
	for name, mine := range map[string][]string{
		"ReadOnly": sorted(reviewRules.ReadOnly), "Unsafe": reviewRules.Unsafe,
		"ReadOnlyPowerShell": sorted(reviewRules.ReadOnlyPowerShell), "UnsafePowerShell": reviewRules.UnsafePowerShell,
	} {
		if !slices.Equal(mine, lists[name]) {
			t.Errorf("%s differs from review.py:\n go %v\npy %v", name, mine, lists[name])
		}
	}
}

// Commands for comparing the Go rules (and Python's shlex) with review.py.
var reviewCorpus = []string{
	"ls -la", "cat a.txt | grep x | wc -l", "find . -name '*.py' && echo done", "pwd", "rm -rf x", "cat a > b",
	"echo $(whoami)", "ls; python3 x.py", "sed -i s/a/b/ f", "cat `ls`", "curl http://x", "find . -exec rm {} ;",
	"find . -delete", `awk 'BEGIN{system("rm x")}'`, "Get-ChildItem -Recurse | Select-String foo",
	`gc C:\work\a.txt | Measure-Object -Line`, "dir; Get-Date", "Remove-Item x", "gci | Remove-Item", "gc (Remove-Item x)",
	"& notepad", "gci | where { Remove-Item $_ }", "Get-Content a > b", "iex 'rm x'", "Invoke-WebRequest http://x",
	`echo "a b`, `echo 'a b`, `ls \`, `cat "a\"b" | head`, `'ls' -la`, `"ls"`, `""`, `'' ls`, "ls\vcat", "ls\fx",
	"ls\x1c rm x", "ls\u2028rm x", "ls\r\nrm x", "\tls\t-l", "", "   ", "|", "||ls", "ls &&", `l"s"`, `l's'`,
	`ls\ -la`, `"l"s`, `'l'"s"`, `echo "it's"`, `echo 'say "hi"'`, `cat "a\\b"`, `cat 'a\b'`, "LS -la", "Dir",
	"GCI|SLS x", "where.exe ls", "ls # rm x", "cat\x00x", "ÿls", "echo ñ | sort", "type a.txt", "test -f x && cat x",
	`"Get-ChildItem"`, `'gci' x`, `gci "C:\Program Files"`, `gci 'C:\x y'`, "Get-ChildItem `", "ls @x", "ls --% x",
	"tee x", "ls|tee x", "ls -ok", "date +%s", "echo a\\", `echo "\$x"`, `echo "\a"`,
}

func TestReviewReadOnlyMatchesReviewPy(t *testing.T) {
	var want struct{ Posix, Windows []bool }
	askPython(t, "print(json.dumps({'Posix': [r.read_only(c, windows=False) for c in data], "+
		"'Windows': [r.read_only(c, windows=True) for c in data]}))", reviewCorpus, &want)
	for i, cmd := range reviewCorpus {
		if got := readOnly(cmd, false); got != want.Posix[i] {
			t.Errorf("POSIX %q: go %v, review.py %v", cmd, got, want.Posix[i])
		}
		if got := readOnly(cmd, true); got != want.Windows[i] {
			t.Errorf("Windows %q: go %v, review.py %v", cmd, got, want.Windows[i])
		}
	}
}

func TestShlexMatchesPython(t *testing.T) {
	type split struct {
		Words []string
		Err   string
	}
	var want struct{ Posix, Windows []split }
	askPython(t, "def s(c, posix):\n"+
		"    try: return {'Words': shlex.split(c, posix=posix), 'Err': ''}\n"+
		"    except ValueError as e: return {'Words': None, 'Err': str(e)}\n"+
		"print(json.dumps({'Posix': [s(c, True) for c in data], 'Windows': [s(c, False) for c in data]}))", reviewCorpus, &want)
	for i, cmd := range reviewCorpus {
		for _, mode := range []struct {
			posix bool
			want  split
		}{{true, want.Posix[i]}, {false, want.Windows[i]}} {
			words, err := shlexSplit(cmd, mode.posix)
			msg := ""
			if err != nil {
				msg = err.Error()
			}
			if msg != mode.want.Err || (err == nil && !slices.Equal(words, mode.want.Words)) {
				t.Errorf("shlex.split(%q, posix=%v): go %q %q, python %q %q", cmd, mode.posix, words, msg, mode.want.Words, mode.want.Err)
			}
		}
	}
	var quoted []string
	words := []string{"", "abc", "a b", "it's", "a@b%c+d=e:f,g./h-i_j", "é", "$x", "~/x"}
	askPython(t, "print(json.dumps([shlex.quote(w) for w in data]))", words, &quoted)
	for i, w := range words {
		if got := shlexQuote(w); got != quoted[i] {
			t.Errorf("shlex.quote(%q): go %q, python %q", w, got, quoted[i])
		}
	}
}

func TestReviewPyAsTheWorkersHook(t *testing.T) {
	// review.py itself as --review: the worker passes the request, the
	// hook's level, pause and question files work as they do in Python.
	if runtime.GOOS == "windows" {
		t.Skip("POSIX quoting in the test")
	}
	py, hook := python3(t), reviewPy(t)
	f := newWorkerFixture(t)
	t.Setenv("MYOUS_HOME", f.home)
	t.Setenv("MYOUS_APPROVAL_WAIT", "2")
	os.Unsetenv("MYOUS_PAUSE_FILE") // review.py takes a set but empty one as "no pause file"
	f.w.reviewCmd = shlexQuote(py) + " " + shlexQuote(hook)
	setLevel(t, f.home, "changes")
	if r := f.exec("ls"); r["exit"] != 0.0 {
		t.Fatalf("%v", r)
	}
	if r := f.exec("touch x"); r["error"] != "the owner didn't answer in time" {
		t.Fatalf("%v", r)
	}
	os.WriteFile(filepath.Join(f.home, "worker.paused"), nil, 0o600)
	if r := f.exec("ls"); r["error"] != pausedByOwner {
		t.Fatalf("%v", r)
	}
}

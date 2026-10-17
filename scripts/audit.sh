#!/bin/sh
# Check the clients' dependencies against known-vulnerability databases.
# Exits non-zero if any check finds something.
# Tools (pinned): govulncheck via `go run`; pip-audit and cargo-audit are
# installed on first use (pip-audit into /tmp/myous-audit-tools, cargo-audit
# into cargo's bin; the workflows cache it under CARGO_AUDIT_VERSION, so
# bump the version here and the cache follows).
set -u
cd "$(dirname "$0")/.."
TOOLS=/tmp/myous-audit-tools
status=0
run() {
	echo "== $1"
	shift
	"$@" || status=1
}

run "govulncheck go" sh -c "cd go && go run golang.org/x/vuln/cmd/govulncheck@v1.8.0 ./..."

if [ ! -x "$TOOLS/py/bin/pip-audit" ]; then
	# A venv left in /tmp can outlive the Python it was made from (a macOS
	# or tools update), and re-creating over it doesn't restore pip: start
	# clean, with the newest Python 3 around, and stop here if that fails.
	rm -rf "$TOOLS/py"
	PY=python3
	for cand in python3.13 python3.12 python3.11 python3.10; do
		command -v "$cand" >/dev/null 2>&1 && { PY=$cand; break; }
	done
	"$PY" -m venv --clear "$TOOLS/py" && "$TOOLS/py/bin/pip" install -q "pip-audit==2.9.0" \
		|| { echo "audit: couldn't set up pip-audit in $TOOLS/py (Python: $PY)" >&2; exit 1; }
fi
# The locks are universal (uv pip compile --universal): lines for some
# platforms only carry markers, and spake2 has one line per path (RELEASING.md,
# "The Intel Mac path"). pip-audit skips lines whose markers don't match the
# machine it runs on and refuses a package twice, so each lock is split into
# files that hold every line once, markers dropped, and each file is audited.
# The split fails (and so the audit) on a line it can't read, if it would
# leave out any line, and on a lock with none.
SPLIT='
import pathlib, re, sys
lock, out = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
text = lock.read_text()
wanted = sum(1 for line in text.splitlines() if line.strip() and not line[0].isspace() and not line.startswith("#"))
views, written = [], 0
for block in re.split(r"\n(?=\S)", text):
    if not block.strip() or block.startswith("#"):
        continue
    m = re.match(r"([A-Za-z0-9_.-]+)==[^\s;\\]+(?: ;[^\\\n]*)? \\\n", block)
    if not m:
        sys.exit(f"{lock}: not a pinned line with hashes: {block.splitlines()[0]}")
    view = next((v for v in views if m.group(1).lower() not in v), None)
    if view is None:
        view = {}
        views.append(view)
    view[m.group(1).lower()] = re.sub(r"^(\S+) ;[^\\\n]*\\", r"\1 \\", block, count=1)
    written += 1
if not wanted or written != wanted:
    sys.exit(f"{lock}: {wanted} requirement lines, {written} split out")
out.mkdir(parents=True, exist_ok=True)
for i, view in enumerate(views):
    path = out / f"{lock.stem}-{i + 1}.txt"
    path.write_text("\n".join(view.values()) + "\n")
    print(path)
'
rm -rf "$TOOLS/lock-views"
for lock in python/requirements.lock python/requirements-qr.lock; do
	views=$("$TOOLS/py/bin/python" -c "$SPLIT" "$lock" "$TOOLS/lock-views") \
		|| { echo "audit: couldn't split $lock" >&2; status=1; continue; }
	for view in $views; do
		run "pip-audit $lock ($(basename "$view"))" "$TOOLS/py/bin/pip-audit" --require-hashes --disable-pip -r "$view"
	done
done

run "npm audit typescript" sh -c "cd typescript && npm audit --omit=dev"
run "npm audit desktop" sh -c "cd worker/desktop && npm audit"

CARGO=${CARGO:-$HOME/.cargo/bin/cargo}
CARGO_AUDIT_VERSION=0.22.2
if ! "$HOME/.cargo/bin/cargo-audit" --version 2>/dev/null | grep -qx "cargo-audit $CARGO_AUDIT_VERSION"; then
	"$CARGO" install --locked --force cargo-audit --version "$CARGO_AUDIT_VERSION" >/dev/null 2>&1 \
		|| { echo "audit: couldn't install cargo-audit $CARGO_AUDIT_VERSION" >&2; exit 1; }
fi
run "cargo audit rust" sh -c "cd rust && $CARGO audit"
# Two allowed warnings here (not vulnerabilities): proc-macro-error is
# unmaintained (RUSTSEC-2024-0370), and glib 0.18 has an unsound iterator
# nothing in the tree calls (RUSTSEC-2024-0429); both come with Tauri 2's
# GTK bindings (gtk 0.18) and go away when Tauri moves them.
run "cargo audit desktop" sh -c "cd worker/desktop/src-tauri && $CARGO audit"

[ $status = 0 ] && echo "all client checks clean" || echo "some client checks reported problems"
exit $status

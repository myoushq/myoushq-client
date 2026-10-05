#!/bin/sh
# Check the clients' dependencies against known-vulnerability databases.
# Exits non-zero if any check finds something.
# Tools (pinned): govulncheck via `go run`; pip-audit and cargo-audit are
# installed on first use (pip-audit into /tmp/myous-audit-tools).
set -u
cd "$(dirname "$0")/.."
TOOLS=/tmp/myous-audit-tools
status=0
run() {
	echo "== $1"
	shift
	"$@" || status=1
}

run "govulncheck go" sh -c "cd go && go run golang.org/x/vuln/cmd/govulncheck@v1.1.4 ./..."

if [ ! -x "$TOOLS/py/bin/pip-audit" ]; then
	python3 -m venv "$TOOLS/py" && "$TOOLS/py/bin/pip" install -q "pip-audit==2.9.0"
fi
for lock in python/requirements.lock python/requirements-qr.lock; do
	run "pip-audit $lock" "$TOOLS/py/bin/pip-audit" --require-hashes --disable-pip -r "$lock"
done

run "npm audit typescript" sh -c "cd typescript && npm audit --omit=dev"

CARGO=${CARGO:-$HOME/.cargo/bin/cargo}
if [ ! -x "$HOME/.cargo/bin/cargo-audit" ]; then
	"$CARGO" install --locked cargo-audit >/dev/null 2>&1
fi
run "cargo audit rust" sh -c "cd rust && $CARGO audit"

[ $status = 0 ] && echo "all client checks clean" || echo "some client checks reported problems"
exit $status

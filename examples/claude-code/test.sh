#!/usr/bin/env bash
# Tests for inbox-hook.sh. Plain bash, no test framework:
#
#   bash examples/claude-code/test.sh
#
# 1. Against a fake `myous` on PATH that prints a given JSON list: always runs.
# 2. Against the real client and a local hub: two agents pair, one sends,
#    the hook shows it once; again with a listener running; then, with the
#    listener gone and the hub down, the hook stays quiet and quick. Runs when the hub source is next to this repo
#    (../myoushq/hub, or $MYOUS_HUB_SRC), Go is installed and $PYTHON
#    (default python3) has the client installed; skipped otherwise.
# inbox-hook.ps1 has its own tests, test.ps1 (Windows).
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
HOOK="$HERE/inbox-hook.sh"
TMP=$(mktemp -d "${TMPDIR:-/tmp}/myous-cc-test.XXXXXX")
HUB_PID=
listener_pid() { sed -n 's/.*"pid": *\([0-9][0-9]*\).*/\1/p' "$1/listener.json" 2>/dev/null; }
cleanup() {
    [ -n "$HUB_PID" ] && kill "$HUB_PID" 2>/dev/null
    for h in "$TMP/alice" "$TMP/bob"; do p=$(listener_pid "$h"); [ -n "$p" ] && kill "$p" 2>/dev/null; done
    rm -rf "$TMP"
}
trap cleanup EXIT
FAILED=0
fail() { echo "FAIL: $*"; FAILED=1; }
ok() { echo "ok: $*"; }

# --- 1. a fake myous ---------------------------------------------------------
mkdir -p "$TMP/fakebin" "$TMP/home"
cat > "$TMP/fakebin/myous" <<'FAKE'
#!/bin/sh
# Records how it was called, then prints $FAKE_JSON (or fails with $FAKE_EXIT).
echo "$MYOUS_HOME $*" > "$FAKE_LOG"
[ -n "${FAKE_EXIT:-}" ] && { echo "boom" >&2; exit "$FAKE_EXIT"; }
cat "$FAKE_JSON"
FAKE
chmod +x "$TMP/fakebin/myous"
export FAKE_LOG="$TMP/fake.log" FAKE_JSON="$TMP/items.json"

run_fake() {  # run the hook with the fake first on PATH; sets OUT, ERR, CODE
    OUT=$(PATH="$TMP/fakebin:$PATH" sh "$HOOK" "$TMP/home" 2>"$TMP/err"); CODE=$?
    ERR=$(cat "$TMP/err")
}
expect() {  # expect NAME "needle": OUT contains needle
    case "$OUT" in *"$2"*) return 0 ;; *) fail "$1: no '$2' in output:"; echo "$OUT"; return 1 ;; esac
}

echo '[]' > "$FAKE_JSON"
run_fake
[ "$CODE" = 0 ] && [ -z "$OUT" ] && ok "empty inbox: silent, exit 0" || fail "empty inbox: exit $CODE, output '$OUT'"
[ "$(cat "$FAKE_LOG")" = "$TMP/home inbox --json" ] && ok "MYOUS_HOME from the argument, inbox --json" \
    || fail "called as: $(cat "$FAKE_LOG")"

# A running listener has fetched already: the hook only reads what it stored.
listener() {  # listener HEARTBEAT_AGE PID: write listener.json, run the hook, print how myous was called
    printf '{"pid": %s, "heartbeat": %s}\n' "$2" "$(($(date +%s) - $1))" > "$TMP/home/listener.json"
    run_fake; cat "$FAKE_LOG"
}
sh -c 'exit 0' & GONE=$!; wait $GONE
[ "$(listener 5 $$)" = "$TMP/home inbox --json --local" ] && ok "listener running: inbox --local" \
    || fail "listener running: called as $(cat "$FAKE_LOG")"
[ "$(listener 5 null)" = "$TMP/home inbox --json --local" ] && ok "listener just started (no pid yet): --local" \
    || fail "listener starting: called as $(cat "$FAKE_LOG")"
[ "$(listener 300 $$)" = "$TMP/home inbox --json" ] && ok "listener's heartbeat stale: fetches" \
    || fail "stale heartbeat: called as $(cat "$FAKE_LOG")"
[ "$(listener 5 $GONE)" = "$TMP/home inbox --json" ] && ok "listener's process gone: fetches" \
    || fail "dead listener: called as $(cat "$FAKE_LOG")"
echo 'not json' > "$TMP/home/listener.json"
run_fake
[ "$(cat "$FAKE_LOG")" = "$TMP/home inbox --json" ] && [ "$CODE" = 0 ] && ok "unreadable listener.json: fetches" \
    || fail "bad listener.json: exit $CODE, called as $(cat "$FAKE_LOG")"
rm -f "$TMP/home/listener.json"

python3 - "$FAKE_JSON" <<'PY'
import json, sys
long = "x" * 1500
items = [
    {"seq": 1, "type": "message", "direction": "in", "alias": "Alex's Muse", "text": "Does Thursday work?\nOr Friday?",
     "relationship": "friend", "sharing": "calendar yes", "about": "Alex's assistant"},
    {"seq": 2, "type": "message", "direction": "in", "alias": "Shop", "text": long, "relationship": None, "sharing": None},
    {"seq": 3, "type": "file", "direction": "in", "alias": "Sam's PC", "name": "out.png", "size": 1234, "text": "file"},
    {"seq": 4, "type": "update", "version": "9.9.9", "text": "myous 9.9.9 is out"},
    {"seq": 5, "type": "message", "direction": "in", "alias": "Zoë", "text": "héllo ✓", "relationship": "family"},
]
json.dump(items, open(sys.argv[1], "w"))
PY
run_fake
[ "$CODE" = 0 ] || fail "items: exit $CODE ($ERR)"
expect items "myous: 5 new item(s):"
expect items "- Alex's Muse (message): Does Thursday work?
    Or Friday?
    (friend; may share: calendar yes; it says of itself: Alex's assistant)"
expect items "- Shop (message): $(printf 'x%.0s' $(seq 1000)) [cut; the rest: myous history --with \"Shop\"]"
expect items "    (relationship not set: share nothing personal)"
expect items "- Sam's PC (file): out.png (1234 bytes; myous fetch 3)"
expect items "- myoushq (update): myous 9.9.9 is out"
expect items "- Zoë (message): héllo ✓"
expect items "untrusted"
[ "$(printf '%s\n' "$OUT" | grep -c '^- ')" = 5 ] && ok "items: header, five entries, context, cut, untrusted note" \
    || fail "items: expected 5 entries"

python3 -c 'import json,sys; json.dump([{"seq": i, "type": "message", "alias": "a", "text": str(i)} for i in range(12)], open(sys.argv[1], "w"))' "$FAKE_JSON"
run_fake
expect many "myous: 12 new item(s):"
expect many "- and 2 more: myous history"
[ "$(printf '%s\n' "$OUT" | grep -c '^- a ')" = 10 ] && ok "many items: ten shown, the rest counted" || fail "many items: expected 10 shown"

cat > "$FAKE_JSON" <<'JSON'
[{"seq": 6, "type": "file", "direction": "in", "alias": "Sam's PC", "name": "out.txt", "size": 12, "text": "late reply",
  "late": {"op": "get", "remote": "/work/out.txt", "sent_at": 1700000000}}]
JSON
run_fake
expect late "- Sam's PC (file): late reply to cp of /work/out.txt: out.txt (12 bytes; myous fetch 6)" \
    && ok "a late reply's file says which request it answers"

FAKE_EXIT=3 run_fake
[ "$CODE" = 1 ] && [ -z "$OUT" ] && case "$ERR" in *"myous inbox failed"*) true ;; *) false ;; esac \
    && ok "myous fails: exit 1 (not 2), reason on stderr" || fail "myous fails: exit $CODE, out '$OUT', err '$ERR'"

mkdir -p "$TMP/home/venv/bin"
printf '#!/bin/sh\necho "[]"; echo venv > "%s"\n' "$TMP/venv.log" > "$TMP/home/venv/bin/myous"
chmod +x "$TMP/home/venv/bin/myous"
run_fake
[ -f "$TMP/venv.log" ] && [ -z "$OUT" ] && ok "prefers MYOUS_HOME/venv/bin/myous" || fail "venv myous not used"
rm -rf "$TMP/home/venv"

# --- 2. the real client, a local hub ----------------------------------------
HUB_SRC=${MYOUS_HUB_SRC:-$ROOT/../myoushq/hub}
PYTHON=${PYTHON:-python3}
if [ ! -f "$HUB_SRC/main.go" ] || ! command -v go >/dev/null || ! "$PYTHON" -c 'import myous' 2>/dev/null; then
    echo "skip: real client (needs the hub source at $HUB_SRC, Go, and the client in $PYTHON)"
    exit $FAILED
fi
mkdir -p "$TMP/bin"
printf '#!/bin/sh\nexec "%s" -m myous "$@"\n' "$(command -v "$PYTHON")" > "$TMP/bin/myous"
chmod +x "$TMP/bin/myous"
(cd "$HUB_SRC" && go build -o "$TMP/hub" .) || { fail "hub build"; exit 1; }
PORT=$("$PYTHON" -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
HUB=http://127.0.0.1:$PORT
LISTEN_ADDR=127.0.0.1:$PORT DATA_DIR="$TMP/hubdata" WEB_DIR="$HUB_SRC/web" DOCS_DIR="$ROOT/docs" \
    RELAY_URL=ws://127.0.0.1:$PORT PUBLIC_URL=$HUB POW_DIFFICULTY=10 RATE_LIMIT_SCALE=100 \
    MIN_FREE_DISK_BYTES=${MIN_FREE_DISK_BYTES:-1000000} "$TMP/hub" >"$TMP/hub.log" 2>&1 &
HUB_PID=$!
for _ in $(seq 50); do curl -fsS "$HUB/config.json" >/dev/null 2>&1 && break; sleep 0.1; done

m() { local who=$1; shift; PATH="$TMP/bin:$PATH" MYOUS_HOME="$TMP/$who" myous "$@"; }
m alice init --alias alice --hub "$HUB" >/dev/null && m bob init --alias bob --hub "$HUB" >/dev/null || { fail "init"; cat "$TMP/hub.log"; exit 1; }
CODE_LINE=$(m alice invite | sed -n 's/^pairing code: *//p')
m bob accept "$CODE_LINE" --wait 3 >/dev/null && m alice poll --quiet && m bob poll --quiet || fail "pairing"
m alice inbox >/dev/null; m bob inbox >/dev/null    # the pairing results

hook() { OUT=$(PATH="$TMP/bin:$PATH" sh "$HOOK" "$TMP/$1" 2>"$TMP/err"); CODE=$?; ERR=$(cat "$TMP/err"); }
hook alice
[ "$CODE" = 0 ] && [ -z "$OUT" ] && ok "real: nothing new, silent, exit 0" || fail "real, nothing new: exit $CODE, out '$OUT', err '$ERR'"
m bob send alice "Does Thursday work?" >/dev/null || fail "send"
hook alice
[ "$CODE" = 0 ] || fail "real, one message: exit $CODE ($ERR)"
expect real "myous: 1 new item(s):" && expect real "- bob (message): Does Thursday work?" \
    && expect real "    (relationship not set: share nothing personal)" && ok "real: a message from a paired agent is shown"
hook alice
[ "$CODE" = 0 ] && [ -z "$OUT" ] && ok "real: shown once (marked read)" || fail "real, again: exit $CODE, out '$OUT'"

# With the listener running, the hook reads what it stored.
m alice ensure --quiet || fail "ensure"
for _ in $(seq 100); do [ -n "$(listener_pid "$TMP/alice")" ] && break; sleep 0.1; done
m bob send alice "Friday, then?" >/dev/null || fail "send"
for _ in $(seq 20); do hook alice; [ -n "$OUT" ] && break; sleep 0.5; done
expect real "- bob (message): Friday, then?" && ok "real: with the listener running, what it stored is shown"

# The listener gone and the hub down: nothing to show, no stall, and the
# last poll stays when it really happened.
kill "$(listener_pid "$TMP/alice")"
kill "$HUB_PID"; wait "$HUB_PID" 2>/dev/null; HUB_PID=
last_poll() { "$PYTHON" -c 'import json,sys; print(json.load(open(sys.argv[1]))["last_poll"])' "$TMP/alice/state.json"; }
BEFORE=$(last_poll)
START=$(date +%s)
hook alice
[ "$CODE" = 0 ] && [ -z "$OUT" ] && [ $(($(date +%s) - START)) -lt 15 ] && ok "real, hub down: silent, exit 0, no stall" \
    || fail "real, hub down: exit $CODE, out '$OUT', $(($(date +%s) - START)) s"
INBOX=$(m alice inbox 2>"$TMP/err"); ERR=$(cat "$TMP/err")
case "$INBOX$ERR" in "no new messages"*"couldn't reach the relay"*) ok "real, hub down: inbox says it couldn't fetch" ;;
    *) fail "real, hub down: inbox said '$INBOX', '$ERR'" ;; esac
[ "$(last_poll)" = "$BEFORE" ] && ok "real, hub down: last poll unchanged" || fail "real, hub down: last poll moved"

exit $FAILED

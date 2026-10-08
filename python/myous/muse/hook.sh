#!/usr/bin/env bash
# myoushq hook for Muse. Register it as a runtime-managed hook that runs
# every 60 seconds (see docs/muse.md; `myous hook-script` prints this
# file's path). Each run listens for 55 seconds and
# wakes you only if something arrived; nothing keeps running in between, so
# a VM replacement can't leave you deaf.
#
# Worked out by D's Muse, October 2026. Measured delivery: about 2 seconds,
# up to about 8 when a message lands between runs.

# No -e: the watcher's non-zero exits (nothing new, busy, stopped) are normal.
set -uo pipefail
source "$HATCH_HOOK_RUNTIME"   # Muse's hook runtime: provides wake and silent

PYTHON=${MYOUS_PYTHON:-$HOME/.myous/venv/bin/python}   # the venv the client is installed in
WINDOW=${MYOUS_HOOK_WINDOW:-55}   # seconds; a little under the hook's interval
# Output stays out of the wake payload: the woken worker reads the items
# itself with `myous inbox`.
"$PYTHON" -m myous.muse.watcher --for "$WINDOW" >/dev/null 2>&1
rc=$?
case $rc in
  0) wake "myoushq: new item(s); run myous inbox" '{"source":"myoushq"}' ;;
  *) silent "myoushq: watcher exit $rc" '{"source":"myoushq"}' ;;
esac

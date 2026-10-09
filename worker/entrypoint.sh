#!/bin/sh
# Container entry point: a virtual display with a VNC view of it, the
# worker's browser, then the worker itself. Everything logs to stdout, so
# `docker compose logs` shows all of it.
#
# The display is TigerVNC's Xvnc (an X server and VNC server in one): it
# accepts the viewer's size, so the browser tab that shows it is filled
# edge to edge at native resolution (noVNC's "remote" resize) instead of a
# scaled fixed screen. matchbox, a minimal window manager, keeps Chromium's
# window maximized, so it follows those size changes.
set -u
# On stop: end the children (Chromium closes its profile cleanly on TERM),
# give them a moment, then exit.
trap 'kill 0 2>/dev/null; sleep 2; exit 0' TERM INT

# Where the worker is, for the owner's app: a phase in the status file
# until the worker itself takes over writing it (starting, browser,
# registering, then the worker's running or paused).
phase() {
	python3 - "$1" "${MYOUS_ALIAS:-Desktop worker}" <<'PY'
import json, os, sys, time
home = os.environ.get("MYOUS_HOME", os.path.expanduser("~/.myous"))
path = os.path.join(home, "worker.json")
try:
    status = json.load(open(path))
except (OSError, ValueError):
    status = {}
status.update(phase=sys.argv[1], alias=status.get("alias") or sys.argv[2], updated=int(time.time()), pid=None)
tmp = path + ".tmp"
with open(tmp, "w") as f:
    json.dump(status, f, indent=2, sort_keys=True)
os.replace(tmp, path)
PY
}
phase starting

export DISPLAY=:99
# No VNC password: the port is published on the host's loopback only
# (compose.yml), so only someone already on this machine can reach it.
Xvnc :99 -geometry 1440x900 -depth 24 -rfbport 5900 -localhost -SecurityTypes None -AlwaysShared \
	-AcceptSetDesktopSize -desktop myous >/dev/null 2>&1 &
sleep 1
matchbox-window-manager -use_titlebar no -use_cursor yes >/dev/null 2>&1 &
# noVNC: the VNC view in a browser tab. A copy of the client is served
# with the client itself as the index page, so the bare address opens it
# and never shows a directory listing.
NOVNC=/tmp/novnc
rm -rf "$NOVNC" && cp -r /usr/share/novnc "$NOVNC"
# The tab's title: "myous - <paired agent>" (the worker's own alias until
# it is paired). A small script in the page reads title.json, which the
# loop below refreshes from the worker's contacts.
sed -i 's/const PAGE_TITLE = "noVNC";/const PAGE_TITLE = "myous";/' "$NOVNC/app/ui.js"
cat > "$NOVNC/title.js" <<'JS'
(function () {
  async function refresh() {
    try {
      const r = await fetch("title.json", { cache: "no-store" });
      if (r.ok) { const t = (await r.json()).title; if (t && document.title !== t) document.title = t; }
    } catch (e) { /* worker not up yet */ }
  }
  // noVNC sets its own title when the connection comes up, so keep ours
  // winning: often at first, then every 10 s.
  refresh(); setInterval(refresh, 2000); setTimeout(() => setInterval(refresh, 10000), 30000);
})();
JS
sed -i 's#</body>#<script src="title.js"></script></body>#' "$NOVNC/vnc.html"
# The client is the site's index, so the address is just
# http://localhost:<port>/?autoconnect=1&reconnect=1&resize=remote
cp "$NOVNC/vnc.html" "$NOVNC/index.html"
(
	while true; do
		python3 - > "$NOVNC/title.json.tmp" 2>/dev/null <<'PY' && mv "$NOVNC/title.json.tmp" "$NOVNC/title.json"
import json, os, subprocess
def run(*a):
    try: return json.loads(subprocess.run(["myous", *a], capture_output=True, text=True, timeout=10).stdout or "null")
    except Exception: return None
contacts = run("contacts", "--json") or {}
paired = [c.get("alias") for c in contacts.values() if c.get("status") == "approved" and c.get("alias")]
alias = paired[0] if paired else ((run("status", "--json") or {}).get("alias") or "worker")
print(json.dumps({"title": "myous - " + alias}))
PY
		sleep 10
	done
) &
websockify --web "$NOVNC" 6080 localhost:5900 >/dev/null 2>&1 &

phase browser
python3 /opt/worker/browser.py &

# Identity: created once, kept in the mounted home (~/.myous-worker on the
# host). `init` also registers with the hub, so it needs the network.
# MYOUS_HUB points at another hub (tests); unset means myoushq.com.
HUB_OPT=${MYOUS_HUB:+--hub "$MYOUS_HUB"}
registered() {
	myous status --json 2>/dev/null | python3 -c 'import json, sys; sys.exit(0 if json.load(sys.stdin).get("registered") else 1)'
}
phase registering
until registered; do
	# shellcheck disable=SC2086
	# init creates the key once and registers; after the key exists,
	# running it again only retries the registration.
	myous init --alias "${MYOUS_ALIAS:-Desktop worker}" $HUB_OPT && continue
	echo "worker: init failed (no network? hub down?); retrying in 10 s"
	phase "error: can't reach the hub; retrying"
	sleep 10
done
# A new name from the owner (MYOUS_ALIAS changed): re-register with it.
current=$(myous status --json 2>/dev/null | python3 -c 'import json, sys; print(json.load(sys.stdin).get("alias") or "")')
ALIAS_OPT=
[ -n "${MYOUS_ALIAS:-}" ] && [ "$current" != "$MYOUS_ALIAS" ] && ALIAS_OPT="--alias"

# The worker loop. If it exits (crash, hub unreachable for long), start it
# again rather than taking the browser down with it. It runs in the
# background and the script waits: a trap only fires while the shell is
# waiting, so with the loop in the foreground `docker stop` would time
# out and kill everything, leaving Chromium's profile lock behind.
(
	while true; do
		# shellcheck disable=SC2086
		myous worker --work /work --review /opt/worker/review.py ${ALIAS_OPT:+$ALIAS_OPT "$MYOUS_ALIAS"}
		echo "worker: exited with status $?; restarting in 10 s"
		phase "error: the worker stopped; restarting"
		ALIAS_OPT=
		sleep 10
	done
) &
wait

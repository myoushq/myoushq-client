#!/bin/sh
# Container entry point: a virtual display with a VNC view of it, the
# worker's browser, then the worker itself. Everything logs to stdout, so
# `docker compose logs` shows all of it.
set -u
trap 'kill 0' TERM INT

export DISPLAY=:99
Xvfb :99 -screen 0 1440x900x24 -nolisten tcp >/dev/null 2>&1 &
sleep 1
# No VNC password: the port is published on the host's loopback only
# (compose.yml), so only someone already on this machine can reach it.
x11vnc -display :99 -forever -shared -nopw -localhost -quiet >/dev/null 2>&1 &
# noVNC: the VNC view in a browser tab. Serve a copy of the client with an
# index page that goes straight to it, connected and scaled, so the bare
# port never shows a directory listing.
NOVNC=/tmp/novnc
rm -rf "$NOVNC" && cp -r /usr/share/novnc "$NOVNC"
cat > "$NOVNC/index.html" <<'HTML'
<!doctype html><meta charset="utf-8"><meta http-equiv="refresh" content="0; url=vnc.html?autoconnect=1&reconnect=1&resize=scale">
<title>Myous Worker browser</title><a href="vnc.html?autoconnect=1&reconnect=1&resize=scale">Open the worker's browser</a>
HTML
websockify --web "$NOVNC" 6080 localhost:5900 >/dev/null 2>&1 &

python3 /opt/worker/browser.py &

# Identity: created once, kept in the mounted home (~/.myous-worker on the
# host). `init` also registers with the hub, so it needs the network.
# MYOUS_HUB points at another hub (tests); unset means myoushq.com.
HUB_OPT=${MYOUS_HUB:+--hub "$MYOUS_HUB"}
registered() {
	myous status --json 2>/dev/null | python3 -c 'import json, sys; sys.exit(0 if json.load(sys.stdin).get("registered") else 1)'
}
until registered; do
	# shellcheck disable=SC2086
	# init creates the key once and registers; after the key exists,
	# running it again only retries the registration.
	myous init --alias "${MYOUS_ALIAS:-Desktop worker}" $HUB_OPT && continue
	echo "worker: init failed (no network? hub down?); retrying in 10 s"
	sleep 10
done

# The worker loop. If it exits (crash, hub unreachable for long), start it
# again rather than taking the browser down with it.
while true; do
	myous worker --work /work --review /opt/worker/review.py
	echo "worker: exited with status $?; restarting in 10 s"
	sleep 10
done

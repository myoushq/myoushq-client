#!/bin/bash
# Starts myous for Linux under Xvfb and checks that its window opens and
# shows something: the window maps, the app is still running after a while,
# and the window area has more than a handful of colours (a blank web view
# has one or two). Takes screenshots and keeps the app's output.
#
# Runs inside the smoke-test container (run.sh) or directly on a Linux
# machine or CI runner with Xvfb, x11-utils, ImageMagick, dbus-x11 and
# (for CLICK) xdotool installed.
#
# Environment:
#   APP      the program to start (required)
#   OUT      where the results go (default /out)
#   TAG      prefix of the result files (default "run")
#   SHOTS    seconds after the window maps to take screenshots (default "10")
#   CLICK    "x y": click there after the last screenshot, then one more
#   XDISPLAY the X display number to use (default 99)
# Exit status: 0 when every check passes.
set -u
: "${APP:?APP is the program to start}"
OUT=${OUT:-/out}; TAG=${TAG:-run}; SHOTS=${SHOTS:-10}; N=${XDISPLAY:-99}
mkdir -p "$OUT"
# No GPU under Xvfb: WebKitGTK's compositing and DMA-BUF renderer need one.
export WEBKIT_DISABLE_COMPOSITING_MODE=1 WEBKIT_DISABLE_DMABUF_RENDERER=1
export NO_AT_BRIDGE=1 RUST_BACKTRACE=1
Xvfb ":$N" -screen 0 1280x800x24 -nolisten tcp >"$OUT/$TAG-xvfb.log" 2>&1 &
XPID=$!
export DISPLAY=":$N"
for _ in $(seq 1 50); do xdpyinfo >/dev/null 2>&1 && break; sleep 0.2; done
eval "$(dbus-launch --sh-syntax)"
ms() { echo $(( $(date +%s%N) / 1000000 )); }

T0=$(ms)
"$APP" >"$OUT/$TAG-app.log" 2>&1 &
PID=$!
WIN='"myous": ("myous-desktop"'
mapped=no
for _ in $(seq 1 120); do
  if xwininfo -root -tree 2>/dev/null | grep -q "$WIN"; then mapped=yes; break; fi
  kill -0 $PID 2>/dev/null || break
  sleep 0.5
done
T1=$(ms)
prev=0
for s in $SHOTS; do
  sleep $((s - prev)); prev=$s
  import -window root "$OUT/$TAG-${s}s.png"
done
alive=no; kill -0 $PID 2>/dev/null && alive=yes
xwininfo -root -tree >"$OUT/$TAG-windows.txt" 2>&1

# Colours in the window's area of the last screenshot.
colours=0
if [ $mapped = yes ]; then
  geo=$(xwininfo -name myous 2>/dev/null | awk '
    /Absolute upper-left X/ {x=$NF} /Absolute upper-left Y/ {y=$NF}
    /Width:/ {w=$NF} /Height:/ {h=$NF} END {printf "%sx%s+%s+%s", w, h, x, y}')
  colours=$(convert "$OUT/$TAG-${prev}s.png" -crop "$geo" +repage -format '%k' info: 2>/dev/null || echo 0)
fi

if [ -n "${CLICK:-}" ] && [ $alive = yes ]; then
  # shellcheck disable=SC2086
  xdotool mousemove $CLICK click 1
  sleep 4
  import -window root "$OUT/$TAG-click.png"
fi
ps -o pid,rss,etime,args -u "$(id -u)" >"$OUT/$TAG-ps.txt"

ok=yes
[ $mapped = yes ] || ok=no
[ $alive = yes ] || ok=no
[ "$colours" -gt 20 ] 2>/dev/null || ok=no
{
  echo "window_mapped=$mapped after_ms=$((T1 - T0))"
  echo "alive_after_${prev}s=$alive"
  echo "window_colours=$colours"
  echo "ok=$ok"
} | tee "$OUT/$TAG-result.txt"

kill $PID 2>/dev/null; sleep 1
kill "$DBUS_SESSION_BUS_PID" $XPID 2>/dev/null
[ $ok = yes ]

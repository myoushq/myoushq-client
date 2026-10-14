#!/bin/sh
# Regenerates the app icons that are files in the repository from
# mark.svg: the icons of myous for Linux and Windows
# (worker/desktop/src-tauri/icons, with `tauri icon`). Re-runnable: it
# replaces those files each time.
#
# myous for Mac needs nothing from here: its app icon and menu bar icon
# are drawn in code from the same geometry (worker/mac/src/Icon.m), and
# make-app.sh packs the app icon. Change the mark in both places.
#
#   worker/icons/make-icons.sh            # needs Node (npm ci in worker/desktop)
#   MYOUS_PW_PYTHON=/path/to/python ...   # a Python with Playwright, if
#                                         # rsvg-convert is not installed
set -eu
cd "$(dirname "$0")"
HERE=$(pwd)
DESKTOP=$HERE/../desktop
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# 1. The mark at 1024 by 1024 pixels, with transparent corners.
PNG=$TMP/mark-1024.png
if command -v rsvg-convert >/dev/null 2>&1; then
	rsvg-convert -w 1024 -h 1024 mark.svg -o "$PNG"
else
	PY=${MYOUS_PW_PYTHON:-/tmp/pw-venv/bin/python}
	[ -x "$PY" ] || { echo "need rsvg-convert, or a Python with Playwright in MYOUS_PW_PYTHON" >&2; exit 1; }
	"$PY" - "$HERE/mark.svg" "$PNG" <<'PY'
import sys
from pathlib import Path
from playwright.sync_api import sync_playwright
svg = Path(sys.argv[1]).read_text()
with sync_playwright() as p:
    b = p.chromium.launch()
    page = b.new_page(viewport={"width": 1024, "height": 1024})
    page.set_content("<style>html,body{margin:0;background:transparent}svg{display:block;width:1024px;height:1024px}</style>" + svg)
    page.screenshot(path=sys.argv[2], omit_background=True)
    b.close()
PY
fi

# 2. myous for Linux and Windows: every size the bundler and toast.rs use
# (PNGs, .ico, .icns, the Windows Store logos). `tauri icon` also writes
# Android and iOS sets, which the app does not ship: dropped.
[ -x "$DESKTOP/node_modules/.bin/tauri" ] || (cd "$DESKTOP" && npm ci --no-audit --no-fund)
OUT=$TMP/tauri
(cd "$DESKTOP" && node_modules/.bin/tauri icon "$PNG" -o "$OUT") >"$TMP/tauri.log" 2>&1 || { cat "$TMP/tauri.log" >&2; exit 1; }
rm -rf "$OUT/android" "$OUT/ios"
ICONS=$DESKTOP/src-tauri/icons
rm -rf "$ICONS"
mkdir -p "$ICONS"
cp "$OUT"/* "$ICONS"/
echo "wrote $(ls "$ICONS" | wc -l | tr -d ' ') files in worker/desktop/src-tauri/icons"

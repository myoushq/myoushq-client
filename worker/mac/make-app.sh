#!/bin/sh
# Build "Myous Worker.app" from source with the Command Line Tools only.
#   worker/mac/make-app.sh [--direct]
# Result: worker/mac/build/Myous Worker.app (drag it to /Applications if you
# like). Records this checkout's path in ~/.myous-worker/app.json so the app
# knows where `docker compose` runs; --direct sets mode "direct" instead
# (the worker runs on this Mac, no Docker).
set -eu
cd "$(dirname "$0")"
MODE=docker
[ "${1:-}" = "--direct" ] && MODE=direct
REPO=$(cd ../.. && pwd)

# Objective-C with clang: the Command Line Tools build it without Xcode, and
# without SwiftPM, whose toolchain can be out of step with the SDK.
mkdir -p build
BIN=build/MyousWorker
clang -fobjc-arc -O2 -Wall -mmacosx-version-min=13.0 -framework Cocoa -framework CoreImage \
	src/*.m -o "$BIN"

APP="build/Myous Worker.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/MyousWorker"

# Icon: the binary draws it (no image files in the repo), iconutil packs it.
ICONSET=build/icon.iconset
rm -rf "$ICONSET"
"$BIN" --render-icon "$ICONSET"
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/MyousWorker.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key><string>Myous Worker</string>
	<key>CFBundleDisplayName</key><string>Myous Worker</string>
	<key>CFBundleIdentifier</key><string>com.myoushq.worker</string>
	<key>CFBundleVersion</key><string>1</string>
	<key>CFBundleShortVersionString</key><string>0.1</string>
	<key>CFBundleExecutable</key><string>MyousWorker</string>
	<key>CFBundleIconFile</key><string>MyousWorker</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>LSMinimumSystemVersion</key><string>13.0</string>
	<key>LSUIElement</key><false/>
	<key>NSHighResolutionCapable</key><true/>
	<key>NSPrincipalClass</key><string>NSApplication</string>
</dict>
</plist>
PLIST

# Ad-hoc signature: enough for a locally built app. Gatekeeper may still ask
# the first time (right-click, Open).
codesign --force --sign - "$APP"

# Tell the app where this checkout is and which mode to use.
mkdir -p ~/.myous-worker && chmod 700 ~/.myous-worker
python3 - "$REPO" "$MODE" <<'PY'
import json, os, sys
p = os.path.expanduser("~/.myous-worker/app.json")
cfg = {}
try:
    cfg = json.load(open(p))
except (OSError, ValueError):
    pass
cfg.update(repo=sys.argv[1], mode=sys.argv[2])
json.dump(cfg, open(p, "w"), indent=2, sort_keys=True)
PY
echo "built $APP (mode $MODE, repo $REPO)"

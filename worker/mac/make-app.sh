#!/bin/sh
# Build "Myous Worker.app" from source with the Command Line Tools only.
#   worker/mac/make-app.sh [--direct] [--version X.Y.Z] [--sign "Developer ID Application: ..."] [--no-config]
# Result: worker/mac/build/Myous Worker.app (drag it to /Applications if you
# like). By default it records this checkout's path in
# ~/.myous-worker/app.json so Start runs `docker compose` there; --direct
# sets mode "direct" (the worker runs on this Mac, no Docker); --no-config
# leaves app.json alone (release builds: the app then uses its built-in
# image). --version sets the app version, which names the image tag
# (default: the version in python/pyproject.toml). --sign signs with a
# Developer ID certificate and the hardened runtime, for notarization;
# otherwise the signature is ad-hoc.
set -eu
cd "$(dirname "$0")"
MODE=docker
WRITE_CONFIG=1
SIGN=
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' ../../python/pyproject.toml | head -1)
while [ $# -gt 0 ]; do
	case "$1" in
	--direct) MODE=direct ;;
	--no-config) WRITE_CONFIG=0 ;;
	--version) VERSION=$2; shift ;;
	--sign) SIGN=$2; shift ;;
	*) echo "unknown option $1" >&2; exit 2 ;;
	esac
	shift
done
[ -n "$VERSION" ] || { echo "no version found; pass --version" >&2; exit 1; }
REPO=$(cd ../.. && pwd)

# Objective-C with clang: the Command Line Tools build it without Xcode, and
# without SwiftPM, whose toolchain can be out of step with the SDK.
mkdir -p build
BIN=build/MyousWorker
# Universal binary (Apple Silicon and Intel), for macOS 12 and newer: the
# runners that build releases are Apple Silicon, and the download must
# run on older Intel Macs too.
clang -fobjc-arc -O2 -Wall -Wunguarded-availability -arch arm64 -arch x86_64 -mmacosx-version-min=12.0 \
	-framework Cocoa -framework CoreImage src/*.m -o "$BIN"

APP="build/Myous Worker.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/MyousWorker"

# Icon: the binary draws it (no image files in the repo), iconutil packs it.
ICONSET=build/icon.iconset
rm -rf "$ICONSET"
"$BIN" --render-icon "$ICONSET"
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/MyousWorker.icns"

# The published image, pinned to this version, for the no-checkout mode.
sed "s/@VERSION@/$VERSION/" compose-image.yml > "$APP/Contents/Resources/compose.yml"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key><string>Myous Worker</string>
	<key>CFBundleDisplayName</key><string>Myous Worker</string>
	<key>CFBundleIdentifier</key><string>com.myoushq.worker</string>
	<key>CFBundleVersion</key><string>$VERSION</string>
	<key>CFBundleShortVersionString</key><string>$VERSION</string>
	<key>CFBundleExecutable</key><string>MyousWorker</string>
	<key>CFBundleIconFile</key><string>MyousWorker</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>LSMinimumSystemVersion</key><string>12.0</string>
	<key>LSUIElement</key><false/>
	<key>NSHighResolutionCapable</key><true/>
	<key>NSPrincipalClass</key><string>NSApplication</string>
	<key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
</dict>
</plist>
PLIST

if [ -n "$SIGN" ]; then
	# Developer ID: hardened runtime and a timestamp are what notarization
	# wants. The app only spawns /bin/sh and docker, so no entitlements.
	codesign --force --options runtime --timestamp --sign "$SIGN" "$APP"
else
	# Ad-hoc signature: enough for a locally built app. Gatekeeper may
	# still ask the first time (right-click, Open).
	codesign --force --sign - "$APP"
fi
codesign --verify --deep --strict --verbose=1 "$APP"

if [ "$WRITE_CONFIG" = 1 ]; then
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
	echo "built $APP $VERSION (mode $MODE, repo $REPO)"
else
	echo "built $APP $VERSION (${SIGN:+signed as $SIGN}${SIGN:-ad-hoc signed}, app.json untouched)"
fi

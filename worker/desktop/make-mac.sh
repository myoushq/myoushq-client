#!/bin/sh
# Build "myous.app" (myous for Mac) from this Tauri app (see MAC-PORT.md),
# then notarize and pack it with worker/mac's scripts, which take any .app.
#   worker/desktop/make-mac.sh [--universal] [--sign "Developer ID Application: ..."]
#                              [--key-id ID --issuer UUID --key AuthKey.p8] [--no-dmg]
# Result: worker/desktop/build/myous.app and build/myous-<version>.dmg
# (with its line in build/SHA256SUMS).
#   --universal  Apple Silicon and Intel in one app (needs the Rust targets
#                aarch64-apple-darwin and x86_64-apple-darwin); otherwise
#                this Mac's architecture only.
#   --sign       Developer ID, hardened runtime and a timestamp, the worker
#                inside first, as worker/mac/make-app.sh signs; otherwise an
#                ad-hoc signature, enough to run it here.
#   --key-id/--issuer/--key  notarize and staple (worker/mac/notarize.sh);
#                needs --sign.
# Tauri's own signing is skipped (--no-sign): it would look in the
# keychain for an identity, and the app is signed here instead, the same
# way for both apps.
set -eu
cd "$(dirname "$0")"
UNIVERSAL=0
SIGN=
KEY_ID='' ISSUER='' KEY=''
DMG=1
while [ $# -gt 0 ]; do
	case "$1" in
	--universal) UNIVERSAL=1 ;;
	--sign) SIGN=$2; shift ;;
	--key-id) KEY_ID=$2; shift ;;
	--issuer) ISSUER=$2; shift ;;
	--key) KEY=$2; shift ;;
	--no-dmg) DMG=0 ;;
	*) echo "unknown option $1" >&2; exit 2 ;;
	esac
	shift
done
[ "$(uname -s)" = Darwin ] || { echo "make-mac.sh builds on a Mac" >&2; exit 2; }
if [ -n "$KEY_ID" ] && [ -z "$SIGN" ]; then
	echo "notarizing needs --sign" >&2
	exit 2
fi
VERSION=$(sed -n 's/^  "version": "\(.*\)"/\1/p' package.json | head -1)
[ -n "$VERSION" ] || { echo "no version in package.json" >&2; exit 1; }
# tauri build, rustc and rustup come from PATH: cargo's directory goes in
# front when $CARGO names one, or when cargo isn't on PATH (rustup's
# default place). A cargo found on PATH changes nothing (its dirname, ".",
# must not go on PATH).
CARGO=${CARGO:-cargo}
command -v "$CARGO" >/dev/null 2>&1 || CARGO=$HOME/.cargo/bin/cargo
case $CARGO in
*/*) PATH=$(dirname "$CARGO"):$PATH; export PATH ;;
esac

# The worker the app carries (the Go client), for the same targets as
# the app: Tauri takes binaries/myous-worker-<target>.
if [ "$UNIVERSAL" = 1 ]; then
	TARGET=universal-apple-darwin
	command -v rustup >/dev/null 2>&1 \
		|| { echo "--universal needs rustup (for the targets aarch64-apple-darwin and x86_64-apple-darwin)" >&2; exit 1; }
	INSTALLED=$(rustup target list --installed)
	for t in aarch64-apple-darwin x86_64-apple-darwin; do
		printf '%s\n' "$INSTALLED" | grep -qx "$t" \
			|| { echo "--universal needs the Rust target $t (rustup target add $t)" >&2; exit 1; }
	done
	# tauri build compiles each architecture on its own, and each
	# compile's build script copies binaries/myous-worker-<that target>;
	# the bundle then takes the -universal-apple-darwin one. So all
	# three, the universal one joined from the other two.
	for t in aarch64-apple-darwin x86_64-apple-darwin; do
		sh ../build-worker.sh "$t" src-tauri/binaries "$VERSION"
	done
	lipo -create -output src-tauri/binaries/myous-worker-universal-apple-darwin \
		src-tauri/binaries/myous-worker-aarch64-apple-darwin src-tauri/binaries/myous-worker-x86_64-apple-darwin
	TARGET_ARGS="--target universal-apple-darwin"
else
	TARGET=$(rustc -vV | sed -n 's/^host: //p')
	sh ../build-worker.sh host src-tauri/binaries "$VERSION"
	TARGET_ARGS=
fi

[ -d node_modules ] || npm ci
# shellcheck disable=SC2086 # TARGET_ARGS is empty or two words
npx tauri build --bundles app --no-sign --ci $TARGET_ARGS

OUTDIR=${CARGO_TARGET_DIR:-src-tauri/target}
if [ "$UNIVERSAL" = 1 ]; then
	BUILT=$OUTDIR/universal-apple-darwin/release/bundle/macos/myous.app
else
	BUILT=$OUTDIR/release/bundle/macos/myous.app
fi
[ -d "$BUILT" ] || { echo "no app at $BUILT" >&2; exit 1; }
mkdir -p build
APP=build/myous.app
rm -rf "$APP"
ditto "$BUILT" "$APP"

WORKER=$APP/Contents/MacOS/myous-worker
if [ "$UNIVERSAL" = 1 ]; then
	# Both programs must run on Apple Silicon and Intel.
	for f in "$APP/Contents/MacOS/myous-desktop" "$WORKER"; do
		archs=" $(lipo -archs "$f") "
		for a in arm64 x86_64; do
			case "$archs" in
			*" $a "*) ;;
			*) echo "$f is not universal (it has$archs, no $a)" >&2; exit 1 ;;
			esac
		done
	done
fi
if [ -n "$SIGN" ]; then
	# What notarization wants: hardened runtime and a timestamp, the
	# worker first (the app's signature seals it). The app spawns only
	# the worker, docker and the owner's browser, so no entitlements.
	codesign --force --options runtime --timestamp --identifier com.myoushq.desktop.worker --sign "$SIGN" "$WORKER"
	codesign --force --options runtime --timestamp --sign "$SIGN" "$APP"
else
	# Ad-hoc: enough for a build run on this Mac. Gatekeeper asks the
	# first time on another one (right-click, Open).
	codesign --force --identifier com.myoushq.desktop.worker --sign - "$WORKER"
	codesign --force --sign - "$APP"
fi
codesign --verify --deep --strict --verbose=1 "$APP"

if [ -n "$KEY_ID" ]; then
	../mac/notarize.sh "$APP" --key-id "$KEY_ID" --issuer "$ISSUER" --key "$KEY"
fi
if [ "$DMG" = 1 ]; then
	if [ -n "$SIGN" ]; then
		../mac/make-dmg.sh "$APP" "build/myous-$VERSION.dmg" --sign "$SIGN"
	else
		../mac/make-dmg.sh "$APP" "build/myous-$VERSION.dmg"
	fi
fi
lipo -archs "$APP/Contents/MacOS/myous-desktop" | sed "s/^/myous.app ($TARGET): /"
echo "built $APP $VERSION"

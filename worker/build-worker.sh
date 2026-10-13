#!/bin/sh
# Build the Go worker (go/cmd/myous) that the myous apps carry inside
# themselves, so a worker without a container needs no Python.
#   worker/build-worker.sh TARGET OUTDIR [VERSION]
# TARGET is a Rust target triple, as Tauri names its bundled programs
# (x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu,
# x86_64-pc-windows-msvc, aarch64-pc-windows-msvc, x86_64-apple-darwin,
# aarch64-apple-darwin, universal-apple-darwin: both Mac architectures
# joined with lipo), or "host" for this computer (as Rust names it, so a
# Tauri build from source finds it). Writes
# OUTDIR/myous-worker-TARGET (".exe" on Windows): never "myous", which on
# Windows is the app itself. VERSION, when given, is the app's version;
# the build fails unless the Go client is the same release.
# Go: the toolchain go/go.mod pins (the go command fetches it).
set -eu
TARGET=${1:?usage: build-worker.sh TARGET OUTDIR [VERSION]}
OUTDIR=${2:?usage: build-worker.sh TARGET OUTDIR [VERSION]}
VERSION=${3:-}
GO=${GO:-go}
command -v "$GO" >/dev/null 2>&1 || { [ -x /usr/local/go/bin/go ] && GO=/usr/local/go/bin/go; } \
	|| { echo "build-worker.sh: no go command (install Go, or set GO)" >&2; exit 1; }
SRC=$(cd "$(dirname "$0")/../go" && pwd)

GOVERSION=$(sed -n 's/^const Version = "\(.*\)"/\1/p' "$SRC/notices.go")
[ -n "$GOVERSION" ] || { echo "build-worker.sh: no Version in go/notices.go" >&2; exit 1; }
if [ -n "$VERSION" ] && [ "$VERSION" != "$GOVERSION" ]; then
	echo "build-worker.sh: the app is $VERSION but the Go worker is $GOVERSION (go/notices.go)" >&2
	exit 1
fi

if [ "$TARGET" = host ]; then
	case "$("$GO" env GOOS)/$("$GO" env GOARCH)" in
	linux/amd64) TARGET=x86_64-unknown-linux-gnu ;;
	linux/arm64) TARGET=aarch64-unknown-linux-gnu ;;
	windows/amd64) TARGET=x86_64-pc-windows-msvc ;;
	windows/arm64) TARGET=aarch64-pc-windows-msvc ;;
	darwin/amd64) TARGET=x86_64-apple-darwin ;;
	darwin/arm64) TARGET=aarch64-apple-darwin ;;
	*) echo "build-worker.sh: no target for this computer; name one" >&2; exit 2 ;;
	esac
fi
EXT=
case "$TARGET" in
x86_64-unknown-linux-gnu) GOOS=linux GOARCH=amd64 ;;
aarch64-unknown-linux-gnu) GOOS=linux GOARCH=arm64 ;;
x86_64-pc-windows-msvc) GOOS=windows GOARCH=amd64 EXT=.exe ;;
aarch64-pc-windows-msvc) GOOS=windows GOARCH=arm64 EXT=.exe ;;
x86_64-apple-darwin) GOOS=darwin GOARCH=amd64 ;;
aarch64-apple-darwin) GOOS=darwin GOARCH=arm64 ;;
universal-apple-darwin) GOOS=darwin GOARCH=universal ;;
*) echo "build-worker.sh: unknown target $TARGET" >&2; exit 2 ;;
esac

mkdir -p "$OUTDIR"
OUT=$(cd "$OUTDIR" && pwd)/myous-worker-$TARGET$EXT
# No cgo: one static program that runs on any glibc (the AppImage) and
# needs nothing next to it. -trimpath keeps this computer's paths out.
build() {
	(cd "$SRC" && CGO_ENABLED=0 GOOS=$GOOS GOARCH=$1 "$GO" build -trimpath -ldflags="-s -w" -o "$2" ./cmd/myous)
}
if [ "$GOARCH" = universal ]; then
	# The Mac app is one universal program; its worker is too. Go 1.27
	# builds for macOS 13 and newer, the app's minimum too.
	TMP=$(mktemp -d)
	trap 'rm -rf "${TMP:?}"' EXIT
	build arm64 "$TMP/arm64"
	build amd64 "$TMP/amd64"
	lipo -create -output "$OUT" "$TMP/arm64" "$TMP/amd64"
else
	build "$GOARCH" "$OUT"
fi
echo "built $OUT (myous $GOVERSION, $GOOS/$GOARCH)"

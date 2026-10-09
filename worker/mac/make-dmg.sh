#!/bin/sh
# Pack "myous.app" (myous for Mac) into a disk image with an Applications shortcut,
# and record its checksum.
#   worker/mac/make-dmg.sh build/myous.app build/myous-0.6.0.dmg [--sign "Developer ID Application: ..."]
# Writes OUT.dmg and appends "<sha256>  <file name>" to SHA256SUMS next to
# it. With --sign the image itself is signed too (a notarized app inside an
# unsigned image still opens; signing the image is tidier).
set -eu
APP=${1:?usage: make-dmg.sh APP OUT.dmg [--sign ID]}
OUT=${2:?usage: make-dmg.sh APP OUT.dmg [--sign ID]}
shift 2
SIGN=
while [ $# -gt 0 ]; do
	case "$1" in
	--sign) SIGN=$2; shift ;;
	*) echo "unknown option $1" >&2; exit 2 ;;
	esac
	shift
done
[ -d "$APP" ] || { echo "no app at $APP" >&2; exit 1; }

STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
# ditto keeps the signature's extended attributes; cp -R wouldn't.
ditto "$APP" "$STAGE/$(basename "$APP")"
ln -s /Applications "$STAGE/Applications"
rm -f "$OUT"
# hdiutil is known to hang or report "Resource busy" on CI runners, so each
# attempt runs under a watchdog and is retried a few times.
with_timeout() {
	secs=$1; shift
	"$@" & cmd=$!
	( sleep "$secs"; kill "$cmd" 2>/dev/null ) & dog=$!
	wait "$cmd"; rc=$?
	kill "$dog" 2>/dev/null; wait "$dog" 2>/dev/null
	return $rc
}
attempt=1
until with_timeout 300 hdiutil create -volname "myous" -srcfolder "$STAGE" -ov -format UDZO -quiet "$OUT"; do
	[ $attempt -lt 5 ] || { echo "hdiutil failed $attempt times" >&2; exit 1; }
	echo "hdiutil attempt $attempt failed; retrying" >&2
	attempt=$((attempt + 1))
	sleep $((attempt * 5))
done
if [ -n "$SIGN" ]; then
	codesign --force --timestamp --sign "$SIGN" "$OUT"
fi
SUMS=$(dirname "$OUT")/SHA256SUMS
( cd "$(dirname "$OUT")" && shasum -a 256 "$(basename "$OUT")" ) >> "$SUMS"
echo "wrote $OUT; checksum appended to $SUMS"

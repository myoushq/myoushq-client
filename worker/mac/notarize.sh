#!/bin/sh
# Notarize and staple a signed "Myous Worker.app" with an App Store Connect
# API key (the kind notarytool takes; no Apple ID password involved).
#   worker/mac/notarize.sh "build/Myous Worker.app" --key-id KEYID --issuer ISSUER_UUID --key AuthKey_KEYID.p8
# The app must already be signed with a Developer ID certificate
# (make-app.sh --sign). Prints Apple's log if notarization fails.
set -eu
APP=${1:?usage: notarize.sh APP --key-id ID --issuer ISSUER --key PATH}
shift
KEY_ID= ISSUER= KEY=
while [ $# -gt 0 ]; do
	case "$1" in
	--key-id) KEY_ID=$2; shift ;;
	--issuer) ISSUER=$2; shift ;;
	--key) KEY=$2; shift ;;
	*) echo "unknown option $1" >&2; exit 2 ;;
	esac
	shift
done
[ -n "$KEY_ID" ] && [ -n "$ISSUER" ] && [ -n "$KEY" ] || { echo "need --key-id, --issuer and --key" >&2; exit 2; }
[ -d "$APP" ] || { echo "no app at $APP" >&2; exit 1; }

# notarytool takes a zip; ditto keeps the bundle's metadata.
ZIP=$(mktemp -d)/app.zip
ditto -c -k --keepParent "$APP" "$ZIP"
OUT=$(mktemp)
if ! xcrun notarytool submit "$ZIP" --key "$KEY" --key-id "$KEY_ID" --issuer "$ISSUER" --wait --output-format json > "$OUT"; then
	cat "$OUT" >&2
	echo "notarytool submit failed" >&2
	exit 1
fi
STATUS=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("status",""))' "$OUT")
ID=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("id",""))' "$OUT")
if [ "$STATUS" != "Accepted" ]; then
	echo "notarization $STATUS (submission $ID); Apple's log:" >&2
	xcrun notarytool log "$ID" --key "$KEY" --key-id "$KEY_ID" --issuer "$ISSUER" >&2 || true
	exit 1
fi
# Staple the ticket so the app opens without a network check.
xcrun stapler staple "$APP"
xcrun stapler validate "$APP"
echo "notarized and stapled: $APP (submission $ID)"

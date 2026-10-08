#!/bin/sh
# Print the changelog section for one release, for the GitHub release body.
#   scripts/release-notes.sh v0.4.0
set -eu
tag=${1:?usage: release-notes.sh vX.Y.Z}
awk -v want="## $tag" '
  $0 == want { on = 1; next }
  on && /^## / { exit }
  on { print }
' "$(dirname "$0")/../docs/changelog.md" | sed -e '1{/^$/d;}'

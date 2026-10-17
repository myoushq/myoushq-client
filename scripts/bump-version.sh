#!/bin/sh
# Bump the release version in every file of RELEASING.md's list (the
# changelog aside), then check the result with check-versions.sh. The
# old version is read from python/pyproject.toml.
#   scripts/bump-version.sh 0.8.1
# Keep this script, check-versions.sh and RELEASING.md step 2 together: a
# file that starts carrying the version goes into all three.
set -eu
new=${1:?usage: bump-version.sh VERSION}
new=${new#v}
cd "$(dirname "$0")/.."
old=$(sed -n 's/^version = "\(.*\)"/\1/p' python/pyproject.toml | head -1)
[ -n "$old" ] || { echo "bump-version: no version in python/pyproject.toml" >&2; exit 1; }
[ "$old" != "$new" ] || { echo "bump-version: already $new" >&2; exit 1; }
echo "bump-version: $old -> $new"
if sed --version >/dev/null 2>&1; then sedi() { sed -i "$@"; }; else sedi() { sed -i '' "$@"; }; fi

sedi "s/^version = \"$old\"/version = \"$new\"/" python/pyproject.toml rust/Cargo.toml worker/desktop/src-tauri/Cargo.toml
sedi "s/^__version__ = \"$old\"/__version__ = \"$new\"/" python/myous/__init__.py
sedi "s/^  \"version\": \"$old\"/  \"version\": \"$new\"/" typescript/package.json worker/desktop/package.json
python3 - "$old" "$new" <<'PY'
import json, sys
old, new = sys.argv[1:]
p = "typescript/npm-shrinkwrap.json"
d = json.load(open(p))
assert d["version"] == old and d["packages"][""]["version"] == old, "npm-shrinkwrap.json is not at the old version"
d["version"] = new
d["packages"][""]["version"] = new
open(p, "w").write(json.dumps(d, indent=2) + "\n")
PY
sedi "s|myous-pake = { path = \"../pake\", version = \"$old\" }|myous-pake = { path = \"../pake\", version = \"$new\" }|" rust/myous/Cargo.toml rust/wasm/Cargo.toml
sedi "s/^const Version = \"$old\"/const Version = \"$new\"/" go/notices.go
sedi "s|@go/v$old|@go/v$new|" go/README.md
sedi "s/--version $old\` for the command, \`myous = \"=$old\"\`/--version $new\` for the command, \`myous = \"=$new\"\`/" rust/README.md
# The lock files: cargo rewrites them without building.
cargo=${CARGO:-$HOME/.cargo/bin/cargo}
command -v "$cargo" >/dev/null 2>&1 || cargo=cargo
(cd rust && "$cargo" update -q --workspace --offline 2>/dev/null || "$cargo" update -q --workspace)
(cd worker/desktop/src-tauri && "$cargo" update -q -p myous-desktop --offline 2>/dev/null || "$cargo" update -q -p myous-desktop)
scripts/check-versions.sh "$new"
echo "bump-version: done; now add v$new to docs/changelog.md (RELEASING.md step 2)"

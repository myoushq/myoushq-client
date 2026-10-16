#!/bin/sh
# Check that every file carrying the release version has VERSION: the
# bump list in RELEASING.md (change the two together). Reads the working
# tree, or REF's files when given (the pre-push hook checks the tag it
# pushes; the release workflow the tag it checked out).
#   scripts/check-versions.sh 0.7.5 [v0.7.5]
# Exits non-zero, naming each file that differs.
set -u
version=${1:?usage: check-versions.sh VERSION [REF]}
version=${version#v}   # v0.7.5 works too
ref=${2:-}
cd "$(dirname "$0")/.." || exit 1
status=0

show() {
	if [ -n "$ref" ]; then git show "$ref:$1"; else cat "$1"; fi
}

# expect FILE AFTER LINE: from the line AFTER on ("" for the top of the
# file), the first line that starts as LINE does up to the version is
# LINE (a trailing comma aside, for JSON).
expect() {
	prefix=${3%%"$version"*}
	got=$(show "$1" 2>/dev/null | awk -v after="$2" -v prefix="$prefix" '
		!on { if (after == "" || $0 == after) on = 1; if (after != "") next }
		index($0, prefix) == 1 { print; exit }')
	if [ "${got%,}" != "$3" ]; then
		echo "check-versions: ${1}${ref:+ at $ref} is not version $version${2:+ (after $2)}: ${got:-no such line}" >&2
		status=1
	fi
}

v=$version
expect python/pyproject.toml '[project]' "version = \"$v\""
expect python/myous/__init__.py '' "__version__ = \"$v\""
expect typescript/package.json '' "  \"version\": \"$v\""
expect typescript/npm-shrinkwrap.json '' "  \"version\": \"$v\""
expect typescript/npm-shrinkwrap.json '    "": {' "      \"version\": \"$v\""
expect rust/Cargo.toml '[workspace.package]' "version = \"$v\""
expect rust/myous/Cargo.toml '[dependencies]' "myous-pake = { path = \"../pake\", version = \"$v\" }"
expect rust/wasm/Cargo.toml '[dependencies]' "myous-pake = { path = \"../pake\", version = \"$v\" }"
for crate in myous myous-pake myous-pake-wasm; do
	expect rust/Cargo.lock "name = \"$crate\"" "version = \"$v\""
done
# The Go client is also the worker the apps carry.
expect go/notices.go '' "const Version = \"$v\""
# The apps for Mac, Linux and Windows all take package.json's.
expect worker/desktop/package.json '' "  \"version\": \"$v\""
expect worker/desktop/src-tauri/Cargo.toml '[package]' "version = \"$v\""
expect worker/desktop/src-tauri/Cargo.lock 'name = "myous-desktop"' "version = \"$v\""
# The install lines.
expect go/README.md '' "go install github.com/myoushq/myoushq-client/go/cmd/myous@go/v$v"
expect rust/README.md '' "--version $v\` for the command, \`myous = \"=$v\"\` as a dependency."

[ $status = 0 ] && echo "check-versions: all at $version${ref:+ in $ref}"
exit $status

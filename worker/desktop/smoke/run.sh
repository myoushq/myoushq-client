#!/bin/bash
# Smoke test of myous for Linux in Docker (also on a Mac): starts the
# AppImage and the .deb under Xvfb, screenshots the window, clicks once.
# See README.md.
#
#   run.sh 0.7.3                          a release, downloaded with gh
#   run.sh --appimage PATH [--deb PATH]   local bundles (either or both)
# Options:
#   --out DIR   results (default: ./myous-smoke-out)
#   --keep      keep the Docker images (removed by default)
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
version="" appimage="" deb="" out="$PWD/myous-smoke-out" keep=no
while [ $# -gt 0 ]; do
  case "$1" in
    --appimage) appimage=$2; shift 2 ;;
    --deb) deb=$2; shift 2 ;;
    --out) out=$2; shift 2 ;;
    --keep) keep=yes; shift ;;
    -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
    -*) echo "unknown option $1" >&2; exit 2 ;;
    *) version=${1#v}; shift ;;
  esac
done
if [ -z "$version" ] && [ -z "$appimage$deb" ]; then
  sed -n '2,12p' "$0" >&2; exit 2
fi
mkdir -p "$out"; out=$(cd "$out" && pwd)
ctx=$(mktemp -d "${TMPDIR:-/tmp}/myous-smoke.XXXXXX")
trap 'rm -rf "$ctx"' EXIT
cp "$here/Dockerfile" "$here/smoke.sh" "$ctx/"

if [ -n "$version" ]; then
  tag="v$version"
  gh release download "$tag" -R myoushq/myoushq-client -D "$ctx" \
    -p "myous-$tag-linux-x86_64.AppImage" -p "myous-$tag-linux-amd64.deb" -p SHA256SUMS
  (cd "$ctx" && grep -E " myous-$tag-linux-(x86_64\.AppImage|amd64\.deb)\$" SHA256SUMS | shasum -a 256 -c -)
  mv "$ctx/myous-$tag-linux-x86_64.AppImage" "$ctx/myous.AppImage"
  mv "$ctx/myous-$tag-linux-amd64.deb" "$ctx/myous.deb"
else
  [ -z "$appimage" ] || cp "$appimage" "$ctx/myous.AppImage"
  [ -z "$deb" ] || cp "$deb" "$ctx/myous.deb"
fi

targets=()
[ ! -f "$ctx/myous.AppImage" ] || targets+=(appimage)
[ ! -f "$ctx/myous.deb" ] || targets+=(deb)
images=()
cleanup_images() { [ $keep = yes ] || [ ${#images[@]} -eq 0 ] || docker rmi "${images[@]}" >/dev/null; }
trap 'cleanup_images; rm -rf "$ctx"' EXIT

failed=0
for t in "${targets[@]}"; do
  echo "== $t"
  start=$(date +%s)
  docker build -q --platform linux/amd64 --target "$t" -t "myous-smoke:$t" "$ctx" >/dev/null
  images+=("myous-smoke:$t")
  echo "built in $(( $(date +%s) - start )) s"
  rm -f "$out/$t"-*
  # SHOTS: right after the window maps and once it has settled.
  # CLICK: "Run without a container" on the first-launch card (no container
  # runtime in here), which opens its warning.
  docker run --rm --platform linux/amd64 --shm-size=256m \
    -e TAG="$t" -e SHOTS="3 10" -e CLICK="533 349" \
    -v "$out:/out" "myous-smoke:$t" smoke.sh || failed=1
  if [ "$t" = deb ]; then
    docker run --rm "myous-smoke:$t" cat /deb-files.txt >"$out/deb-files.txt"
    docker run --rm "myous-smoke:$t" cat /deb-install.log >"$out/deb-install.log"
    echo "deb puts: $(grep -E '^/usr/bin/' "$out/deb-files.txt" | tr '\n' ' ')"
  fi
done
echo "results in $out"
[ $keep = yes ] && echo "images kept: ${images[*]}"
exit $failed

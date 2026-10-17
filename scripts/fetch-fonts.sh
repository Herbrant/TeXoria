#!/usr/bin/env bash
# Download the bundled editor font (IBM Plex Mono, SIL Open Font License)
# into crates/texoria/assets/fonts/.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/crates/texoria/assets/fonts"
URL="https://github.com/IBM/plex/releases/download/%40ibm%2Fplex-mono%402.5.0/ibm-plex-mono.zip"
SHA256=6d23f01257663d8cc49a0d64c22ced630b79e0e2a0ac08a0da86e9a38bbc481c
. "$ROOT/scripts/sha256.sh"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "Downloading $URL"
curl -fsSL --retry 3 --retry-all-errors "$URL" -o "$tmp/plex.zip"
verify_sha256 "$tmp/plex.zip" "$SHA256"
unzip -q "$tmp/plex.zip" -d "$tmp/x"

mkdir -p "$DEST"
for style in Regular Italic Medium SemiBold Bold; do
  f="$(find "$tmp/x" -path '*/fonts/complete/ttf/*' -name "IBMPlexMono-$style.ttf" | head -1)"
  [ -n "$f" ] && cp "$f" "$DEST/"
done
lic="$(find "$tmp/x" -iname 'license*' | head -1)"
[ -n "$lic" ] && cp "$lic" "$DEST/LICENSE-OFL.txt"
ls -l "$DEST"

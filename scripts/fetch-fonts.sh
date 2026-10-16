#!/usr/bin/env bash
# Download the bundled editor font (IBM Plex Mono, SIL Open Font License)
# into crates/texoria/assets/fonts/.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/crates/texoria/assets/fonts"
URL="https://github.com/IBM/plex/releases/download/%40ibm%2Fplex-mono%402.5.0/ibm-plex-mono.zip"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "Downloading $URL"
curl -fsSL "$URL" -o "$tmp/plex.zip"
unzip -q "$tmp/plex.zip" -d "$tmp/x"

mkdir -p "$DEST"
for style in Regular Italic Medium SemiBold Bold; do
  f="$(find "$tmp/x" -path '*/fonts/complete/ttf/*' -name "IBMPlexMono-$style.ttf" | head -1)"
  [ -n "$f" ] && cp "$f" "$DEST/"
done
lic="$(find "$tmp/x" -iname 'license*' | head -1)"
[ -n "$lic" ] && cp "$lic" "$DEST/LICENSE-OFL.txt"
ls -l "$DEST"

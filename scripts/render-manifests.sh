#!/usr/bin/env bash
# Fill the Homebrew cask and AUR PKGBUILD templates with the workspace version and the
# checksums of the release assets.
#   scripts/render-manifests.sh SHA256SUMS OUT_DIR  →  OUT_DIR/texoria.rb, OUT_DIR/PKGBUILD
set -euo pipefail

[ $# -eq 2 ] || { echo "usage: $0 SHA256SUMS OUT_DIR" >&2; exit 1; }
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PKG="$ROOT/crates/texoria/packaging"
SUMS="$1"
OUT="$2"
VERSION="$("$ROOT/scripts/version.sh")"

# sum_of ASSET — the SHA-256 of a release asset, from a `sha256sum` listing.
sum_of() {
  local sha
  sha="$(awk -v f="$1" '{ name = $2; sub(/^\*/, "", name) } name == f { print $1 }' "$SUMS")"
  [ -n "$sha" ] || { echo "no checksum for $1 in $SUMS" >&2; exit 1; }
  echo "$sha"
}

# Assignments, so that a missing checksum stops the script (set -e).
mac_arm="$(sum_of "texoria-$VERSION-macos-aarch64.dmg")"
mac_x64="$(sum_of "texoria-$VERSION-macos-x86_64.dmg")"
linux_x64="$(sum_of "texoria-$VERSION-linux-x86_64.tar.gz")"
linux_arm="$(sum_of "texoria-$VERSION-linux-aarch64.tar.gz")"

mkdir -p "$OUT"
sed -e "s/@VERSION@/$VERSION/g" \
  -e "s/@SHA256_MACOS_AARCH64@/$mac_arm/" -e "s/@SHA256_MACOS_X86_64@/$mac_x64/" \
  "$PKG/homebrew/texoria.rb.in" > "$OUT/texoria.rb"
sed -e "s/@VERSION@/$VERSION/g" \
  -e "s/@SHA256_LINUX_X86_64@/$linux_x64/" -e "s/@SHA256_LINUX_AARCH64@/$linux_arm/" \
  "$PKG/aur/PKGBUILD.in" > "$OUT/PKGBUILD"
if grep -n '@[A-Z0-9_]*@' "$OUT/texoria.rb" "$OUT/PKGBUILD"; then
  echo "unfilled placeholders" >&2
  exit 1
fi
echo "$OUT/texoria.rb"
echo "$OUT/PKGBUILD"

#!/usr/bin/env bash
# Download the official Tectonic CLI for the host into vendor/tectonic/.
# TeXoria bundles it so documents compile even without a TeX distribution.
# On Linux the static musl builds are used: the gnu one needs a recent glibc (2.39).
set -euo pipefail

PINNED=0.17.0
VERSION="${TECTONIC_VERSION:-$PINNED}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/vendor/tectonic"
. "$ROOT/scripts/sha256.sh"

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) triple=aarch64-apple-darwin; ext=tar.gz ;;
  Darwin-x86_64) triple=x86_64-apple-darwin; ext=tar.gz ;;
  Linux-x86_64) triple=x86_64-unknown-linux-musl; ext=tar.gz ;;
  Linux-aarch64) triple=aarch64-unknown-linux-musl; ext=tar.gz ;;
  MINGW*-x86_64|MSYS*-x86_64) triple=x86_64-pc-windows-msvc; ext=zip ;;
  *) echo "unsupported platform: $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac

asset="tectonic-$VERSION-$triple.$ext"
# SHA-256 of the $PINNED assets (checked against GitHub's asset digests).
sha=""
if [ "$VERSION" = "$PINNED" ]; then
  case "$triple" in
    aarch64-apple-darwin) sha=a3f1cac7c5678f01661a92212f58480ae3b0634115d880dbc59e2953ded45667 ;;
    x86_64-apple-darwin) sha=7c90ef5b6ddb1eb1937e4337add5237b79338e4b9676459fa91187d24d6cdf80 ;;
    x86_64-unknown-linux-musl) sha=8533d07f9ccbd7a65824b9e0459041bca34af1eb33daba48f59215593753a3b7 ;;
    aarch64-unknown-linux-musl) sha=b10954a95404f3ab2328d2fa59a5ebab8e657f893fab096f98be8db7c0c979b8 ;;
    x86_64-pc-windows-msvc) sha=f61ce51f0b0ade1015b7de7ef368541c5424e9756ecbd0d7af97d6d48030845f ;;
  esac
fi
url="https://github.com/tectonic-typesetting/tectonic/releases/download/tectonic%40$VERSION/$asset"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "Downloading $url"
curl -fsSL --retry 3 --retry-all-errors "$url" -o "$tmp/$asset"
verify_sha256 "$tmp/$asset" "$sha"
mkdir -p "$DEST"
if [ "$ext" = zip ]; then unzip -q -o "$tmp/$asset" -d "$DEST"; else tar -xzf "$tmp/$asset" -C "$DEST"; fi
"$DEST/tectonic" --version

# The MIT license must ship with the binary in release packages.
license_sha=""
[ "$VERSION" = "$PINNED" ] && license_sha=814a258f76e420b25cb3c07172eb2b3956f34cefbf0a650413b78e65c425f306
curl -fsSL --retry 3 --retry-all-errors "https://raw.githubusercontent.com/tectonic-typesetting/tectonic/tectonic%40$VERSION/LICENSE" \
  -o "$tmp/LICENSE"
verify_sha256 "$tmp/LICENSE" "$license_sha"
cp "$tmp/LICENSE" "$DEST/LICENSE"

#!/usr/bin/env bash
# Download the official Tectonic CLI for the host into vendor/tectonic/.
# TeXoria bundles it so documents compile even without a TeX distribution.
set -euo pipefail

VERSION="${TECTONIC_VERSION:-0.17.0}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/vendor/tectonic"

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) triple=aarch64-apple-darwin; ext=tar.gz ;;
  Darwin-x86_64) triple=x86_64-apple-darwin; ext=tar.gz ;;
  Linux-x86_64) triple=x86_64-unknown-linux-gnu; ext=tar.gz ;;
  Linux-aarch64) triple=aarch64-unknown-linux-musl; ext=tar.gz ;;
  MINGW*-x86_64|MSYS*-x86_64) triple=x86_64-pc-windows-msvc; ext=zip ;;
  *) echo "unsupported platform: $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac

asset="tectonic-$VERSION-$triple.$ext"
url="https://github.com/tectonic-typesetting/tectonic/releases/download/tectonic%40$VERSION/$asset"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "Downloading $url"
curl -fsSL "$url" -o "$tmp/$asset"
mkdir -p "$DEST"
if [ "$ext" = zip ]; then unzip -q -o "$tmp/$asset" -d "$DEST"; else tar -xzf "$tmp/$asset" -C "$DEST"; fi
"$DEST/tectonic" --version

#!/usr/bin/env bash
# Download prebuilt PDFium (bblanchon/pdfium-binaries) for the host platform
# into vendor/pdfium/. The version must match pdfium-render's `pdfium_*` feature.
set -euo pipefail

VERSION="${PDFIUM_VERSION:-7881}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/vendor/pdfium"

case "$(uname -s)" in
  Darwin) os=mac ;;
  Linux) os=linux ;;
  MINGW*|MSYS*|CYGWIN*) os=win ;;
  *) echo "unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  arm64|aarch64) arch=arm64 ;;
  x86_64|amd64) arch=x64 ;;
  *) echo "unsupported arch: $(uname -m)" >&2; exit 1 ;;
esac

asset="pdfium-$os-$arch.tgz"
url="https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F$VERSION/$asset"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "Downloading $url"
curl -fsSL "$url" -o "$tmp/$asset"
mkdir -p "$tmp/x"
tar -xzf "$tmp/$asset" -C "$tmp/x"

rm -rf "$DEST"
mkdir -p "$DEST"
# The library lives in lib/ (macOS/Linux) or bin/ (Windows).
find "$tmp/x" \( -name 'libpdfium.dylib' -o -name 'libpdfium.so' -o -name 'pdfium.dll' \) -exec cp {} "$DEST/" \;
cp "$tmp/x/LICENSE" "$DEST/LICENSE" 2>/dev/null || true
echo "$VERSION" > "$DEST/VERSION"
ls -l "$DEST"

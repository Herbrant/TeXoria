#!/usr/bin/env bash
# Download prebuilt PDFium (bblanchon/pdfium-binaries) for the host platform
# into vendor/pdfium/. The version must match pdfium-render's `pdfium_*` feature.
set -euo pipefail

PINNED=7881
VERSION="${PDFIUM_VERSION:-$PINNED}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/vendor/pdfium"
. "$ROOT/scripts/sha256.sh"

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
# SHA-256 of the $PINNED assets (checked against GitHub's asset digests).
sha=""
if [ "$VERSION" = "$PINNED" ]; then
  case "$asset" in
    pdfium-mac-arm64.tgz) sha=52e94ca5aa8847934330daf3f8150c190682c5ca93831468794f8b90d4392e40 ;;
    pdfium-mac-x64.tgz) sha=6dedf83990e0e3d6b7c93c9e7589c5a126b0ae14b7464d76120cff7a26afb18b ;;
    pdfium-linux-arm64.tgz) sha=ee7f7b7d5468958336a818c1cd580bdd20972846b7377b13f9a923d92d1d4674 ;;
    pdfium-linux-x64.tgz) sha=1470e21b8b4a3b4ad7f85684e2da11d94f3b69a86d81dee11b9b6709d927ac1d ;;
    pdfium-win-arm64.tgz) sha=d3035d4d2cacac6ecd1a2ece197a3d702a1b2a58466276b9f870b8cb278a9d84 ;;
    pdfium-win-x64.tgz) sha=73cc0de638ac2095e7445bf56a38200a5b7c7ca0e9f4ba144598f2457377ac08 ;;
  esac
fi
url="https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F$VERSION/$asset"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "Downloading $url"
curl -fsSL "$url" -o "$tmp/$asset"
verify_sha256 "$tmp/$asset" "$sha"
mkdir -p "$tmp/x"
tar -xzf "$tmp/$asset" -C "$tmp/x"

rm -rf "$DEST"
mkdir -p "$DEST"
# The library lives in lib/ (macOS/Linux) or bin/ (Windows).
find "$tmp/x" \( -name 'libpdfium.dylib' -o -name 'libpdfium.so' -o -name 'pdfium.dll' \) -exec cp {} "$DEST/" \;
cp "$tmp/x/LICENSE" "$DEST/LICENSE" 2>/dev/null || true
echo "$VERSION" > "$DEST/VERSION"
ls -l "$DEST"

#!/usr/bin/env bash
# Rasterize the app icon (assets/logo/*.svg) into the files used for packaging and for the
# Windows executable: PNGs for Linux (hicolor), texoria.icns for macOS, texoria.ico for Windows.
# Sizes up to 48 px use the simplified texoria-small.svg. Requires macOS (iconutil),
# rsvg-convert (`brew install librsvg`) and python3.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/assets/logo"
DEST="$ROOT/crates/texoria/packaging"

for tool in rsvg-convert iconutil python3; do
  command -v "$tool" >/dev/null || { echo "missing tool: $tool" >&2; exit 1; }
done

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# render <size> <out.png>
render() {
  local svg="$SRC/texoria.svg"
  [ "$1" -le 48 ] && svg="$SRC/texoria-small.svg"
  rsvg-convert -w "$1" -h "$1" "$svg" -o "$2"
}

for size in 16 24 32 48 64 128 256 512 1024; do
  render "$size" "$tmp/$size.png"
done

mkdir -p "$DEST/icons"
rm -f "$DEST/icons/"*.png
for size in 16 32 48 64 128 256 512; do
  cp "$tmp/$size.png" "$DEST/icons/${size}x${size}.png"
done

iconset="$tmp/texoria.iconset"
mkdir "$iconset"
for size in 16 32 128 256 512; do
  cp "$tmp/$size.png" "$iconset/icon_${size}x${size}.png"
  cp "$tmp/$((size * 2)).png" "$iconset/icon_${size}x${size}@2x.png"
done
iconutil -c icns "$iconset" -o "$DEST/texoria.icns"

# ICO with PNG-compressed entries (supported since Windows Vista).
python3 - "$DEST/texoria.ico" "$tmp" 16 24 32 48 64 128 256 <<'PY'
import struct, sys
out, tmp, sizes = sys.argv[1], sys.argv[2], [int(s) for s in sys.argv[3:]]
images = [open(f"{tmp}/{s}.png", "rb").read() for s in sizes]
offset = 6 + 16 * len(sizes)
header = struct.pack("<HHH", 0, 1, len(sizes))
entries = b""
for size, data in zip(sizes, images):
    dim = 0 if size == 256 else size
    entries += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
    offset += len(data)
with open(out, "wb") as f:
    f.write(header + entries + b"".join(images))
PY

echo "Icons written to $DEST"

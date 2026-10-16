#!/usr/bin/env bash
# Stage the Linux install tree of a release build and package it into target/dist as
# texoria-<version>-linux-<arch>.tar.gz (used by the AUR package), .deb and .rpm (nfpm).
#
#   usr/bin/texoria
#   usr/lib/texoria/libpdfium.so      (found by pdfium_dirs() in workspace.rs)
#   usr/lib/texoria/bin/tectonic      (found by bundled_tectonic_dirs() in tex-compile)
#   usr/share/{applications,icons/hicolor,licenses/texoria}
#
# Needs `cargo build -p texoria --release`, scripts/fetch-pdfium.sh, scripts/fetch-tectonic.sh,
# readelf and nfpm.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PKG="$ROOT/crates/texoria/packaging"
STAGE="$ROOT/target/stage/root"
DIST="$ROOT/target/dist"
VERSION="$("$ROOT/scripts/version.sh")"

[ "$(uname -s)" = Linux ] || { echo "this script runs on Linux" >&2; exit 1; }
case "$(uname -m)" in
  x86_64) arch=x86_64; nfpm_arch=amd64 ;;
  aarch64) arch=aarch64; nfpm_arch=arm64 ;;
  *) echo "unsupported arch: $(uname -m)" >&2; exit 1 ;;
esac
for f in target/release/texoria vendor/pdfium/libpdfium.so vendor/tectonic/tectonic \
  vendor/pdfium/LICENSE vendor/tectonic/LICENSE; do
  [ -f "$ROOT/$f" ] || { echo "missing $f" >&2; exit 1; }
done

# Every shared library the binaries link must be covered by the package dependencies
# (nfpm.yaml, packaging/aur/PKGBUILD.in): fail when a new one shows up.
allowed='^(libc|libm|libdl|libpthread|librt|libgcc_s|ld-linux-[a-z0-9_-]+|libxcb|libxkbcommon|libxkbcommon-x11|libfontconfig|libfreetype)\.so(\.[0-9]+)*$'
for bin in target/release/texoria vendor/pdfium/libpdfium.so vendor/tectonic/tectonic; do
  needed="$(readelf -d "$ROOT/$bin" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p')"
  echo "$bin: $(echo "$needed" | tr '\n' ' ')"
  bad="$(echo "$needed" | grep -Ev "$allowed" || true)"
  if [ -n "$bad" ]; then
    echo "$bin links libraries missing from the package dependencies: $(echo "$bad" | tr '\n' ' ')" >&2
    exit 1
  fi
done

rm -rf "$STAGE"
install -Dm755 "$ROOT/target/release/texoria" "$STAGE/usr/bin/texoria"
install -Dm755 "$ROOT/vendor/pdfium/libpdfium.so" "$STAGE/usr/lib/texoria/libpdfium.so"
install -Dm755 "$ROOT/vendor/tectonic/tectonic" "$STAGE/usr/lib/texoria/bin/tectonic"
install -Dm644 "$PKG/linux/texoria.desktop" "$STAGE/usr/share/applications/texoria.desktop"
for png in "$PKG"/icons/*.png; do
  size="$(basename "$png" .png)"
  install -Dm644 "$png" "$STAGE/usr/share/icons/hicolor/$size/apps/texoria.png"
done
licenses="$STAGE/usr/share/licenses/texoria"
install -Dm644 "$ROOT/LICENSE" "$licenses/LICENSE"
install -Dm644 "$ROOT/vendor/pdfium/LICENSE" "$licenses/LICENSE.pdfium"
install -Dm644 "$ROOT/vendor/tectonic/LICENSE" "$licenses/LICENSE.tectonic"

mkdir -p "$DIST"
tarball="$DIST/texoria-$VERSION-linux-$arch.tar.gz"
tar --owner=0 --group=0 --numeric-owner -C "$STAGE" -czf "$tarball" usr
echo "$tarball"

cd "$ROOT"
for format in deb rpm; do
  VERSION="$VERSION" NFPM_ARCH="$nfpm_arch" \
    nfpm package --config "$PKG/linux/nfpm.yaml" --packager "$format" --target "$DIST/"
done

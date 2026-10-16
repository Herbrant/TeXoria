#!/usr/bin/env bash
# Sign target/dist/TeXoria.app (built by `cargo packager --formats app`) ad hoc and wrap it in
# target/dist/texoria-<version>-macos-<arch>.dmg. There is no Developer ID, so the app is not
# notarized: the ad-hoc signature only seals the bundle (Apple silicon refuses unsigned code).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIST="$ROOT/target/dist"
APP="$DIST/TeXoria.app"
VERSION="$("$ROOT/scripts/version.sh")"

case "$(uname -m)" in
  arm64) arch=aarch64 ;;
  x86_64) arch=x86_64 ;;
  *) echo "unsupported arch: $(uname -m)" >&2; exit 1 ;;
esac
[ -d "$APP" ] || { echo "missing $APP: run cargo packager --release -p texoria --formats app" >&2; exit 1; }

xattr -cr "$APP"
codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict "$APP"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cp -R "$APP" "$tmp/"
ln -s /Applications "$tmp/Applications"
dmg="$DIST/texoria-$VERSION-macos-$arch.dmg"
rm -f "$dmg"
hdiutil create -volname TeXoria -srcfolder "$tmp" -fs HFS+ -format UDZO -ov "$dmg"
echo "$dmg"

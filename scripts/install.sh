#!/bin/sh
# Install the latest TeXoria release on macOS or Linux, without root:
#
#   curl -fsSL https://raw.githubusercontent.com/Herbrant/TeXoria/main/scripts/install.sh | sh
#
# macOS: TeXoria.app goes to /Applications (~/Applications if that is not writable).
# Linux: the release tarball goes to ~/.local (bin/texoria, lib/texoria, share/...).
# Every download is checked against the release's SHA256SUMS.
#
# Environment:
#   TEXORIA_VERSION   version to install (default: the latest release), e.g. 0.1.0
#   TEXORIA_APP_DIR   macOS: where to put TeXoria.app
#   TEXORIA_PREFIX    Linux: install prefix (default: ~/.local)
set -eu

REPO="Herbrant/TeXoria"

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "this script needs $1"; }

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

# download ASSET: fetch a release asset into $tmp and check it against SHA256SUMS.
download() {
  say "Downloading $1"
  curl -fL --progress-bar "$base/$1" -o "$tmp/$1" || die "could not download $base/$1"
  expected="$(awk -v name="$1" '$2 == name || $2 == "*" name { print $1 }' "$tmp/SHA256SUMS")"
  [ -n "$expected" ] || die "$1 is not listed in SHA256SUMS"
  [ "$(sha256 "$tmp/$1")" = "$expected" ] || die "SHA-256 mismatch for $1"
}

install_macos() {
  arch="$(uname -m)"
  # A shell running under Rosetta reports x86_64 on Apple silicon.
  [ "$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)" = 1 ] && arch=arm64
  case "$arch" in
    arm64) arch=aarch64 ;;
    x86_64) ;;
    *) die "unsupported architecture: $arch" ;;
  esac

  app_dir="${TEXORIA_APP_DIR:-}"
  if [ -z "$app_dir" ]; then
    app_dir=/Applications
    [ -w "$app_dir" ] || app_dir="$HOME/Applications"
  fi
  mkdir -p "$app_dir"

  dmg="texoria-$version-macos-$arch.dmg"
  download "$dmg"
  mnt="$tmp/mnt"
  mkdir -p "$mnt"
  hdiutil attach -nobrowse -readonly -noautoopen -quiet -mountpoint "$mnt" "$tmp/$dmg"
  rm -rf "$app_dir/TeXoria.app"
  ditto "$mnt/TeXoria.app" "$app_dir/TeXoria.app"
  hdiutil detach -quiet "$mnt"
  # The app is signed ad hoc, not notarized: Gatekeeper would refuse a quarantined copy.
  xattr -dr com.apple.quarantine "$app_dir/TeXoria.app" 2>/dev/null || true

  say "Installed TeXoria $version in $app_dir/TeXoria.app"
}

install_linux() {
  case "$(uname -m)" in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) die "unsupported architecture: $(uname -m)" ;;
  esac
  prefix="${TEXORIA_PREFIX:-$HOME/.local}"

  tarball="texoria-$version-linux-$arch.tar.gz"
  download "$tarball"
  mkdir -p "$tmp/x" "$prefix"
  tar -xzf "$tmp/$tarball" -C "$tmp/x"
  # Drop the files of a previous version (PDFium, Tectonic) before copying the new ones.
  rm -rf "$prefix/lib/texoria"
  cp -R "$tmp/x/usr/." "$prefix/"

  # Desktop launchers do not see ~/.local/bin on every system, nor icons outside the usual
  # prefixes: point the menu entry at both directly.
  desktop="$prefix/share/applications/texoria.desktop"
  sed -e "s|^Exec=texoria |Exec=\"$prefix/bin/texoria\" |" \
    -e "s|^Icon=texoria\$|Icon=$prefix/share/icons/hicolor/512x512/apps/texoria.png|" \
    "$tmp/x/usr/share/applications/texoria.desktop" >"$desktop"
  if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q "$prefix/share/applications" 2>/dev/null || true
  fi

  say "Installed TeXoria $version in $prefix"
  case ":$PATH:" in
    *":$prefix/bin:"*) say "Run it from your application menu, or with: texoria" ;;
    *) say "Run it from your application menu, or add $prefix/bin to your PATH to run: texoria" ;;
  esac
}

need curl
need tar
tmp="$(mktemp -d)"
cleanup() {
  [ -d "$tmp/mnt" ] && hdiutil detach -quiet "$tmp/mnt" 2>/dev/null
  rm -rf "$tmp"
}
trap cleanup EXIT
trap 'exit 1' INT TERM

version="${TEXORIA_VERSION:-}"
version="${version#v}"
if [ -z "$version" ]; then
  # /releases/latest redirects to the newest release that is not a pre-release.
  latest="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest")" ||
    die "could not reach GitHub"
  version="${latest##*/v}"
  case "$version" in
    "" | */*) die "could not find the latest release" ;;
  esac
fi
base="https://github.com/$REPO/releases/download/v$version"
curl -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS" || die "no release v$version"

case "$(uname -s)" in
  Darwin) install_macos ;;
  Linux) install_linux ;;
  *) die "unsupported system: $(uname -s) (on Windows, use the installer from the releases page)" ;;
esac

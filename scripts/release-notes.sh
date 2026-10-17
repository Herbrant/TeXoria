#!/usr/bin/env bash
# Print the release notes of a version: its section of CHANGELOG.md, without the heading.
# Fails if the version has no section or the section is empty.
#   scripts/release-notes.sh [version]   (default: the workspace version)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="${1:-$("$ROOT/scripts/version.sh")}"
VERSION="${VERSION#v}"

notes="$(awk -v heading="## [$VERSION]" '
  index($0, "## [") == 1 { if (in_section) exit; in_section = index($0, heading) == 1; next }
  in_section { print }
' "$ROOT/CHANGELOG.md")"

# Drop leading and trailing blank lines.
notes="$(printf '%s\n' "$notes" | sed -e '/./,$!d')"
if [ -z "${notes//[[:space:]]/}" ]; then
  echo "CHANGELOG.md has no notes for $VERSION" >&2
  exit 1
fi
printf '%s\n' "$notes"

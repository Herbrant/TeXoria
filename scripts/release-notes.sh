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

# Drop leading blank lines, and join wrapped lines: GitHub renders every line break of a release
# body, unlike a Markdown file's.
notes="$(printf '%s\n' "$notes" | sed -e '/./,$!d' | awk '
  /^[[:space:]]*$/ { if (line != "") print line; print ""; line = ""; next }
  line != "" && !/^(#|[-*+] |[0-9]+\. |```|\|)/ { sub(/^[[:space:]]+/, ""); line = line " " $0; next }
  { if (line != "") print line; line = $0 }
  END { if (line != "") print line }
')"
if [ -z "${notes//[[:space:]]/}" ]; then
  echo "CHANGELOG.md has no notes for $VERSION" >&2
  exit 1
fi
printf '%s\n' "$notes"

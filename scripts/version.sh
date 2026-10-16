#!/usr/bin/env bash
# Print the workspace version ([workspace.package] in Cargo.toml).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
awk '
  /^\[workspace\.package\]/ { in_pkg = 1; next }
  /^\[/ { in_pkg = 0 }
  in_pkg && /^version *=/ { gsub(/^version *= *"|".*$/, ""); print; found = 1; exit }
  END { if (!found) exit 1 }
' "$ROOT/Cargo.toml"

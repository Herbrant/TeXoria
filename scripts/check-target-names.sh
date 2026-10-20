#!/usr/bin/env bash
# Fail if a workspace target (lib, bin, test, example, bench) has the crate name of a
# dependency's library.
#
#   ./scripts/check-target-names.sh
#
# Before every run, cargo-llvm-cov calls `cargo clean -p` for each workspace member, which
# deletes artifacts by target name: a test named like a dependency's library (tests/scheduler.rs
# and gpui's `scheduler`) deletes that library too, and everything depending on it is rebuilt.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Every platform's dependencies (no --filter-platform), names as rustc sees them.
collisions="$(cargo metadata --format-version 1 --manifest-path "$ROOT/Cargo.toml" | jq -r '
  (.workspace_members) as $ws
  | [ .packages[] | select(.id as $id | $ws | index($id) | not)
      | .name as $pkg | .targets[] | select(.kind | index("lib") or index("rlib")
          or index("proc-macro") or index("dylib") or index("cdylib") or index("staticlib"))
      | { key: (.name | gsub("-"; "_")), value: $pkg } ] as $libs
  | ($libs | group_by(.key) | map({ key: .[0].key, value: (map(.value) | unique) })
      | from_entries) as $dep
  | .packages[] | select(.id as $id | $ws | index($id))
  | .name as $pkg | .targets[]
  | (.name | gsub("-"; "_")) as $name
  | select($dep[$name])
  | "\($pkg): \(.kind | join(",")) target `\(.name)` shares its name with the library of \($dep[$name] | join(", "))"
')"

if [ -n "$collisions" ]; then
  echo "$collisions"
  echo "Rename these targets: cargo-llvm-cov would delete the dependency's artifacts on every run."
  exit 1
fi

#!/usr/bin/env bash
# Check line coverage per crate against scripts/coverage-thresholds.tsv.
#
#   cargo llvm-cov --workspace --json --summary-only --output-path cov.json
#   ./scripts/coverage-check.sh cov.json
#
# Only the crates' `src/` files count (not their tests or test helpers). Prints a Markdown
# table, also appended to $GITHUB_STEP_SUMMARY in CI, and fails if a crate is below its minimum
# or a crate has no minimum.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
JSON="${1:?usage: coverage-check.sh COV.json [THRESHOLDS.tsv]}"
THRESHOLDS="${2:-$ROOT/scripts/coverage-thresholds.tsv}"

# crate<TAB>covered<TAB>count, from the per-file summaries (Windows paths use backslashes).
per_crate="$(jq -r '
  [ .data[0].files[]
    | { file: (.filename | gsub("\\\\"; "/")), lines: .summary.lines }
    | select(.file | test("/crates/[^/]+/src/"))
    | { crate: (.file | capture("/crates/(?<c>[^/]+)/src/").c), lines } ]
  | group_by(.crate)[]
  | [ .[0].crate, (map(.lines.covered) | add), (map(.lines.count) | add) ]
  | @tsv
' "$JSON" | tr -d '\r')"

table="| Crate | Lines | Covered | Minimum |  |
|---|---:|---:|---:|---|"
failed=0
while IFS=$'\t' read -r crate covered count; do
  [ "$crate" = test-support ] && continue
  min="$(awk -F'\t' -v c="$crate" '!/^#/ && $1 == c { print $2 }' "$THRESHOLDS" | tr -d '\r')"
  pct="$(awk -v a="$covered" -v b="$count" 'BEGIN { printf "%.2f", b ? 100 * a / b : 100 }')"
  if [ -z "$min" ]; then
    mark="no minimum in $(basename "$THRESHOLDS")"
    failed=1
  elif awk -v p="$pct" -v m="$min" 'BEGIN { exit !(p + 0 < m + 0) }'; then
    mark="below minimum"
    failed=1
  else
    mark="ok"
  fi
  table+=$'\n'"| $crate | $count | $pct% | ${min:-–}% | $mark |"
done <<< "$per_crate"

echo "$table"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  { echo "### Line coverage ($(uname -s) $(uname -m))"; echo; echo "$table"; } >> "$GITHUB_STEP_SUMMARY"
fi
exit "$failed"

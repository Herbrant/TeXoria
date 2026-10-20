#!/usr/bin/env bash
# Measure TeXoria's memory and CPU while it shows a document, for before/after comparisons.
#
#   cargo build --release -p texoria
#   ./scripts/measure.sh [-s SECONDS] [-w WARMUP] [-b BINARY] path/to/document.tex
#
# Starts the app on the document, waits WARMUP seconds (default 15: the first build, the PDF,
# the indexes), then samples it every second for SECONDS (default 30) and prints the memory
# (macOS: the footprint, what Activity Monitor shows; elsewhere: the RSS) at the end and at
# its highest, and the average CPU over the window. Leave the window alone for an idle
# measure, or type in it for an editing one. The app is quit at the end.
#
# The app runs with its own home folder, target/measure-home: the default settings, and the
# user's own (recent projects, sessions) are left alone. A spelling dictionary downloaded there
# on the first run stays for the next ones.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SECONDS_=30
WARMUP=15
BIN="$ROOT/target/release/texoria"
while getopts "s:w:b:" opt; do
  case "$opt" in
    s) SECONDS_="$OPTARG" ;;
    w) WARMUP="$OPTARG" ;;
    b) BIN="$OPTARG" ;;
    *) exit 2 ;;
  esac
done
shift $((OPTIND - 1))
DOC="${1:?usage: measure.sh [-s SECONDS] [-w WARMUP] [-b BINARY] DOCUMENT}"
[ -x "$BIN" ] || { echo "$BIN not found: cargo build --release -p texoria" >&2; exit 1; }

HOME_="$ROOT/target/measure-home"
mkdir -p "$HOME_"
HOME="$HOME_" XDG_CONFIG_HOME="$HOME_/.config" XDG_DATA_HOME="$HOME_/.local/share" \
  XDG_CACHE_HOME="$HOME_/.cache" "$BIN" "$DOC" >/dev/null 2>&1 &
PID=$!
trap 'kill "$PID" 2>/dev/null || true' EXIT

# Memory in KB.
memory() {
  if [ "$(uname)" = Darwin ]; then
    footprint -p "$PID" 2>/dev/null | awk '
      /Footprint:/ {
        for (i = 1; i <= NF; i++) if ($i == "Footprint:") { v = $(i + 1); u = $(i + 2) }
        if (u == "GB") v *= 1048576; else if (u == "MB") v *= 1024; else if (u == "B") v /= 1024
        printf "%d\n", v; exit
      }'
  else
    awk '/^VmRSS:/ { print $2 }' "/proc/$PID/status"
  fi
}

# CPU time used so far, in seconds (`ps -o time`: [[dd-]hh:]mm:ss[.ss]).
cpu_time() {
  ps -o time= -p "$PID" | awk '{
    t = $1; d = 0
    if (index(t, "-")) { split(t, a, "-"); d = a[1]; t = a[2] }
    n = split(t, p, ":"); s = 0
    for (i = 1; i <= n; i++) s = s * 60 + p[i]
    print s + d * 86400
  }'
}

sleep "$WARMUP"
kill -0 "$PID" 2>/dev/null || { echo "TeXoria exited during the warm-up" >&2; exit 1; }
start_cpu="$(cpu_time)"
peak=0
for _ in $(seq "$SECONDS_"); do
  sleep 1
  m="$(memory)"
  [ "${m:-0}" -gt "$peak" ] && peak="$m"
done
end_cpu="$(cpu_time)"

awk -v m="$m" -v p="$peak" -v a="$start_cpu" -v b="$end_cpu" -v s="$SECONDS_" 'BEGIN {
  printf "memory: %.1f MB (peak %.1f MB)\n", m / 1024, p / 1024
  printf "cpu:    %.1f%% on average over %d s (%.2f s of CPU)\n", 100 * (b - a) / s, s, b - a
}'

#!/usr/bin/env bash
# Measure TeXoria's memory and CPU while it shows a document, for before/after comparisons.
#
#   cargo build --release -p texoria
#   ./scripts/measure.sh [-s SECONDS] [-w WARMUP] [-b BINARY] path/to/document.tex
#
# Starts the app on the document, waits WARMUP seconds (default 60: the first build, the PDF,
# the indexes), then prints its average CPU over the next SECONDS (default 30) and its memory
# at the end. Leave the window alone for an idle measure, or type in it for an editing one. On
# macOS the window is brought to the front after the warm-up (a hidden window draws nothing).
# The app is quit at the end.
#
# On macOS the memory is the footprint (what Activity Monitor shows), the bytes the app holds
# in malloc (its own data, the most stable of the three) and the GPU's memory (textures: the
# PDF's pages, glyphs), from `vmmap --summary`; elsewhere it is the RSS. Nothing is sampled
# during the CPU window: inspecting the app's memory is charged to the app. Not the RSS
# either on macOS, which counts the system's shared libraries (about 600 MB).
#
# The app runs with its own home folder, target/measure-home: the default settings, and the
# user's own (recent projects, sessions) are left alone. A spelling dictionary downloaded there
# on the first run stays for the next ones.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SECONDS_=30
WARMUP=60
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

# The memory lines to print.
memory() {
  if [ "$(uname)" = Darwin ]; then
    vmmap --summary "$PID" 2>/dev/null | awk '
      # Sizes like 12K, 3.5M, 1.2G, in MB.
      function mb(s) {
        u = substr(s, length(s)); v = substr(s, 1, length(s) - 1) + 0
        return u == "G" ? v * 1024 : u == "K" ? v / 1024 : u == "M" ? v : v / 1048576
      }
      /^Physical footprint:/ { footprint = mb($3) }
      /^IOAccelerator \(graphics\)/ { gpu = mb($5) }
      /^MALLOC ZONE/ { zones = 1 }
      zones && /^TOTAL/ { malloc = mb($7) }
      END {
        printf "memory: %.1f MB (footprint)\n", footprint
        printf "        %.1f MB allocated in malloc, %.1f MB of GPU memory\n", malloc, gpu
      }'
  else
    awk '/^VmRSS:/ { printf "memory: %.1f MB (RSS)\n", $2 / 1024 }' "/proc/$PID/status"
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
# A hidden window draws nothing: bring it to the front, as when it is used.
if [ "$(uname)" = Darwin ]; then
  osascript -e "tell application \"System Events\" to set frontmost of \
    (first process whose unix id is $PID) to true" >/dev/null 2>&1 || true
  sleep 2
fi
start_cpu="$(cpu_time)"
sleep "$SECONDS_"
end_cpu="$(cpu_time)"

awk -v a="$start_cpu" -v b="$end_cpu" -v s="$SECONDS_" 'BEGIN {
  printf "cpu:    %.1f%% on average over %d s (%.2f s of CPU)\n", 100 * (b - a) / s, s, b - a
}'
memory

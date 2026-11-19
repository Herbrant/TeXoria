#!/usr/bin/env bash
# Assemble the website (website/) with the assets it shares with the app (logo, screenshots,
# IBM Plex Mono, Lucide icons) and the documentation (docs/*.md as HTML pages, built by
# crates/site-docs through website/doc-template.html) into target/website, which
# .github/workflows/pages.yml publishes to GitHub Pages.
# Preview it with: python3 -m http.server -d target/website
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/target/website"
APP_ASSETS="$ROOT/crates/texoria/assets"

rm -rf "$OUT"
mkdir -p "$OUT/logo" "$OUT/fonts" "$OUT/icons"

for file in "$ROOT"/website/*; do
  [ "$(basename "$file")" = doc-template.html ] || cp "$file" "$OUT/"
done
cp "$ROOT"/assets/logo/texoria.svg "$ROOT"/assets/logo/texoria-small.svg "$OUT/logo/"
cp "$ROOT"/assets/screenshot-paper.png "$ROOT"/assets/screenshot-ink.png "$OUT/"
for weight in Regular Medium SemiBold; do
  cp "$APP_ASSETS/fonts/IBMPlexMono-$weight.ttf" "$OUT/fonts/"
done
cp "$APP_ASSETS/fonts/LICENSE-OFL.txt" "$OUT/fonts/"

# Only the icons that style.css uses.
icons="$(grep -o 'icons/[a-z0-9-]*\.svg' "$ROOT/website/style.css" | sort -u)"
[ -n "$icons" ] || { echo "no icons found in website/style.css" >&2; exit 1; }
for icon in $icons; do
  cp "$APP_ASSETS/$icon" "$OUT/icons/"
done
cp "$APP_ASSETS/icons/LICENSE" "$OUT/icons/"

# docs/*.md → docs/*.html; fails on a broken link or a page missing from docs/README.md.
cargo run --quiet --locked --manifest-path "$ROOT/Cargo.toml" -p site-docs -- \
  "$ROOT/docs" "$ROOT/website/doc-template.html" "$OUT/docs"

# GitHub Pages must serve the files as they are, without Jekyll.
touch "$OUT/.nojekyll"

echo "website built in $OUT"

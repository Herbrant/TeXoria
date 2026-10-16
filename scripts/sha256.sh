# Sourced by the fetch scripts: verify downloads before they are unpacked
# (and later bundled into release packages).

# verify_sha256 FILE EXPECTED — EXPECTED empty means "no pinned hash".
verify_sha256() {
  local file="$1" expected="$2" actual
  if [ -z "$expected" ]; then
    echo "warning: no pinned SHA-256 for $(basename "$file"); skipping verification" >&2
    return 0
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$file" | cut -d' ' -f1)"
  else
    actual="$(shasum -a 256 "$file" | cut -d' ' -f1)"
  fi
  if [ "$actual" != "$expected" ]; then
    echo "SHA-256 mismatch for $(basename "$file"):" >&2
    echo "  expected $expected" >&2
    echo "  actual   $actual" >&2
    return 1
  fi
}

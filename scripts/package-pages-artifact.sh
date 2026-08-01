#!/usr/bin/env bash
set -euo pipefail

root="${1:-dist}"
output="${2:-${RUNNER_TEMP:-artifacts}/github-pages.tar}"
evidence_dir="${3:-artifacts/pages-evidence}"

node scripts/pages-artifact-evidence.mjs --root "$root" --out "$evidence_dir"
mkdir -p "$(dirname "$output")"
rm -f "$output"

# A deterministic archive binds the Pages upload to the exact tree that passed
# source and generated-output checks. We package it ourselves because the
# current official composite action excludes top-level dot-directories, which
# would silently drop RFC 9116 `/.well-known/security.txt`.
LC_ALL=C tar \
  --format=ustar \
  --sort=name \
  --mtime="@${SOURCE_DATE_EPOCH:-0}" \
  --owner=0 \
  --group=0 \
  --numeric-owner \
  --mode='u+rwX,go+rX,go-w' \
  -cf "$output" \
  -C "$root" \
  .

entries_file="$evidence_dir/pages-archive-entries.txt"
LC_ALL=C tar -tf "$output" | LC_ALL=C sort > "$entries_file"

grep -Fxq './index.html' "$entries_file"
grep -Fxq './deployment.json' "$entries_file"
if [[ -f "$root/.well-known/security.txt" ]]; then
  grep -Fxq './.well-known/security.txt' "$entries_file"
fi

sha256sum "$output" > "$evidence_dir/github-pages.tar.sha256"
echo "Packaged deterministic Pages artifact: $output"

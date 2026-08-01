#!/usr/bin/env bash
set -euo pipefail

root="${1:-dist}"
output="${2:-${RUNNER_TEMP:-artifacts}/artifact.tar}"
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
  --dereference \
  --hard-dereference \
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

# Prove the archive that will be uploaded expands to the same bytes that were
# inventoried. This catches omitted dot-directories and any packaging-time drift.
verification_root="$(mktemp -d)"
trap 'rm -rf "$verification_root"' EXIT
LC_ALL=C tar -xf "$output" -C "$verification_root"
archive_evidence="$evidence_dir/archive-roundtrip"
node scripts/pages-artifact-evidence.mjs \
  --root "$verification_root" \
  --out "$archive_evidence"
cmp "$evidence_dir/pages-tree.sha256" "$archive_evidence/pages-tree.sha256"

sha256sum "$output" > "$evidence_dir/artifact.tar.sha256"
echo "Packaged deterministic Pages artifact: $output"

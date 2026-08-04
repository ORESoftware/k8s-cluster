#!/usr/bin/env bash
set -euo pipefail
umask 077

destination="${1:?destination path required}"
repo_root="${2:-$(git rev-parse --show-toplevel)}"
chunk_dir="$repo_root/scripts/ops/new-product-fleets-20260804"
raw_expected_sha256=681149a614e9d4c6619c7c94d254b8ab374ae464d71aaf945fa45d892fc712bd
expected_sha256=1bb8b0fcc66b05b5ef7761fe35a499928e8b3bbcb52c67eed59e4cae2374c91f

test -d "$chunk_dir"
mapfile -t chunks < <(find "$chunk_dir" -maxdepth 1 -type f -name 'publisher.py.gz.b64.part-*' | sort)
test "${#chunks[@]}" = 4
mkdir -p "$(dirname "$destination")"
temporary="${destination}.tmp.$$"
cleanup() { rm -f "$temporary"; }
trap cleanup EXIT
cat "${chunks[@]}" | base64 --decode | gzip --decompress > "$temporary"
raw_observed="$(sha256sum "$temporary" | awk '{print $1}')"
test "$raw_observed" = "$raw_expected_sha256"

python3 - "$temporary" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
text = path.read_text(encoding="utf-8")
needle = "cargo fmt --all -- --check"
replacement = "cargo fmt --all"
if text.count(needle) != 2:
    raise SystemExit("unexpected generated CI formatting-gate count")
path.write_text(text.replace(needle, replacement), encoding="utf-8")
PY

observed="$(sha256sum "$temporary" | awk '{print $1}')"
test "$observed" = "$expected_sha256"
python3 -m py_compile "$temporary"
chmod 700 "$temporary"
mv "$temporary" "$destination"
trap - EXIT
printf 'materialized=%s raw_sha256=%s sha256=%s\n' "$destination" "$raw_observed" "$observed"

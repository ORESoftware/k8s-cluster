#!/usr/bin/env bash
set -Eeuo pipefail
umask 077
output="${1:?output directory required}"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
archive="$(mktemp /tmp/requested-product-fleet-tools.XXXXXX.tar.xz)"
cleanup() { rm -f "$archive"; }
trap cleanup EXIT
mkdir -p "$output"
cat "$script_dir"/requested-product-fleet-tools-20260804.tar.xz.b64.part* \
  | tr -d '\r\n' \
  | base64 --decode \
  > "$archive"
printf '%s  %s\n' '87aa53c3a797d894a70056c8802edd2b3676342e72ede2251bf02caa5e88fddc' "$archive" | sha256sum --check --strict
tar --extract --xz --file "$archive" --directory "$output"
chmod 0755 \
  "$output/generate_streempilot_sp_fleet.py" \
  "$output/finalize_requested_product_fleets.py" \
  "$output/run_protected_requested_product_fleets_20260804.sh"
python3 -m py_compile \
  "$output/generate_streempilot_sp_fleet.py" \
  "$output/finalize_requested_product_fleets.py"
bash -n "$output/run_protected_requested_product_fleets_20260804.sh"

#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

root=""
if [[ "${1:-}" == "--root" ]]; then
  root="${2:?--root requires a path}"
  shift 2
else
  root="$(git rev-parse --show-toplevel)"
fi
[[ $# -eq 0 ]]
root="$(cd "$root" && pwd)"
parts_dir="$root/scripts/ops"
prefix="$parts_dir/elenkos_fleet_payload_20260819.b64.part"
digest_file="$parts_dir/elenkos_fleet_payload_20260819.sha256"
archive="$(mktemp "${TMPDIR:-/tmp}/elenkos-fleet-payload.XXXXXX.tar.gz")"
cleanup() { rm -f "$archive"; }
trap cleanup EXIT

mapfile -t parts < <(find "$parts_dir" -maxdepth 1 -type f -name 'elenkos_fleet_payload_20260819.b64.part*' | sort)
test "${#parts[@]}" -gt 0
cat "${parts[@]}" | base64 --decode > "$archive"
expected="$(tr -d '[:space:]' < "$digest_file")"
actual="$(sha256sum "$archive" | awk '{print $1}')"
[[ "$expected" =~ ^[0-9a-f]{64}$ ]]
[[ "$actual" == "$expected" ]]
tar -xzf "$archive" -C "$root" --no-same-owner --no-same-permissions
python3 "$root/scripts/ops/patch_elenkos_fleet_payload_20260819.py" "$root"

required=(
  scripts/ops/patch_elenkos_fleet_payload_20260819.py
  scripts/ops/elenkos_fleet_spec_20260819.py
  scripts/ops/publish_elenkos_fleet_20260819.py
  scripts/ops/run_protected_elenkos_fleet_20260819.sh
  scripts/ops/dispatch_elenkos_fleet_via_ssm_20260819.sh
  scripts/ops/test_elenkos_fleet_20260819.py
  scripts/ops/validate_elenkos_fleet_payload_20260819.sh
  docs/den-3786-elenkos-fleet.md
)
for relative in "${required[@]}"; do
  [[ -s "$root/$relative" ]] || { echo "missing materialized payload file: $relative" >&2; exit 72; }
done
printf 'ELENKOS_PAYLOAD_MATERIALIZED files=%s sha256=%s\n' "${#required[@]}" "$actual"

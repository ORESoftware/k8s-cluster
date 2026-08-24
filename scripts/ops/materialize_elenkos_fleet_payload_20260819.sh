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
augment_prefix="$parts_dir/augment_elenkos_fleet_20260819.py.gz.b64.part"
augment_target="$parts_dir/augment_elenkos_fleet_20260819.py"
augment_expected="58ea2870c136160847e65e864388e4f85be92954fbdede356d90178eceb36c90"
mirror_patch="$parts_dir/patch_elenkos_mirror_fleet_20260819.py"
empty_repository_patch="$parts_dir/patch_elenkos_empty_repository_main_ref_20260820.py"
tag_visibility_patch="$parts_dir/patch_elenkos_tag_visibility_race_20260820.py"
bootstrap_tag_patch="$parts_dir/patch_elenkos_bootstrap_tag_reconcile_20260820.py"
managed_tree_patch="$parts_dir/patch_elenkos_managed_tree_reconcile_20260820.py"
archive="$(mktemp "${TMPDIR:-/tmp}/elenkos-fleet-payload.XXXXXX.tar.gz")"
augment_tmp="$(mktemp "${TMPDIR:-/tmp}/augment-elenkos-fleet.XXXXXX.py")"
cleanup() { rm -f "$archive" "$augment_tmp"; }
trap cleanup EXIT

mapfile -t parts < <(find "$parts_dir" -maxdepth 1 -type f -name 'elenkos_fleet_payload_20260819.b64.part*' | sort)
test "${#parts[@]}" -gt 0
cat "${parts[@]}" | base64 --decode > "$archive"
expected="$(tr -d '[:space:]' < "$digest_file")"
actual="$(sha256sum "$archive" | awk '{print $1}')"
[[ "$expected" =~ ^[0-9a-f]{64}$ ]]
[[ "$actual" == "$expected" ]]
tar -xzf "$archive" -C "$root" --no-same-owner --no-same-permissions

mapfile -t augment_parts < <(find "$parts_dir" -maxdepth 1 -type f -name 'augment_elenkos_fleet_20260819.py.gz.b64.part*' | sort)
test "${#augment_parts[@]}" -gt 0
cat "${augment_parts[@]}" | base64 --decode | gzip -dc > "$augment_tmp"
augment_actual="$(sha256sum "$augment_tmp" | awk '{print $1}')"
[[ "$augment_actual" == "$augment_expected" ]]
mv "$augment_tmp" "$augment_target"
python3 -m py_compile \
  "$root/scripts/ops/patch_elenkos_fleet_payload_20260819.py" \
  "$augment_target" \
  "$mirror_patch" \
  "$empty_repository_patch" \
  "$tag_visibility_patch" \
  "$bootstrap_tag_patch" \
  "$managed_tree_patch"
python3 "$root/scripts/ops/patch_elenkos_fleet_payload_20260819.py" "$root"
python3 "$mirror_patch" "$root"
python3 "$empty_repository_patch" \
  --publisher "$root/scripts/ops/publish_elenkos_fleet_20260819.py"
python3 "$tag_visibility_patch" \
  --publisher "$root/scripts/ops/publish_elenkos_fleet_20260819.py"
python3 "$bootstrap_tag_patch" \
  --publisher "$root/scripts/ops/publish_elenkos_fleet_20260819.py"
python3 "$managed_tree_patch" \
  --recovery "$root/scripts/ops/recover_elenkos_partial_bootstrap_20260820.py"

required=(
  scripts/ops/patch_elenkos_fleet_payload_20260819.py
  scripts/ops/patch_elenkos_mirror_fleet_20260819.py
  scripts/ops/patch_elenkos_empty_repository_main_ref_20260820.py
  scripts/ops/patch_elenkos_tag_visibility_race_20260820.py
  scripts/ops/patch_elenkos_bootstrap_tag_reconcile_20260820.py
  scripts/ops/patch_elenkos_managed_tree_reconcile_20260820.py
  scripts/ops/augment_elenkos_fleet_20260819.py
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
printf 'ELENKOS_PAYLOAD_MATERIALIZED files=%s payload_sha256=%s augment_sha256=%s repositories=22 production=11 test=11 managed_tree_reconcile=true\n' \
  "${#required[@]}" "$actual" "$augment_actual"

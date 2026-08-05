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

# Keep generated Rust feature commits formatted on every execution host. The
# sealed payload is immutable; this bounded source patch is applied after its
# checksum has been verified and before the generator is compiled or run.
python3 - "$output/generate_streempilot_sp_fleet.py" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
text = path.read_text(encoding="utf-8")
old = '''    run(["git", "checkout", "-b", FEATURE_BRANCH], cwd=root)\n    write_files(root, files_for(spec))\n    run(["git", "add", "."], cwd=root)\n'''
new = '''    run(["git", "checkout", "-b", FEATURE_BRANCH], cwd=root)\n    write_files(root, files_for(spec))\n    if spec.kind != "infra" and shutil.which("cargo"):\n        run(["cargo", "fmt", "--all"], cwd=root)\n    run(["git", "add", "."], cwd=root)\n'''
if new not in text:
    if old not in text:
        raise SystemExit("StreemPilot generator formatting patch target was not found")
    text = text.replace(old, new, 1)

# Serde's enum-level rename_all changes variant names, not fields inside
# struct variants. Add rename_all_fields to the generated RealtimeEvent enum
# so studio_id serializes as studioId, matching the public JSON contract and
# the existing generator test.
marker = "pub enum RealtimeEvent"
marker_index = text.find(marker)
if marker_index < 0:
    raise SystemExit("StreemPilot RealtimeEvent enum patch target was not found")
attribute_start = text.rfind("#[serde(", max(0, marker_index - 1200), marker_index)
attribute_end = text.find(")]", attribute_start, marker_index)
if attribute_start < 0 or attribute_end < 0:
    raise SystemExit("StreemPilot RealtimeEvent serde attribute was not found")
attribute_end += 2
attribute = text[attribute_start:attribute_end]
if 'rename_all_fields = "camelCase"' not in attribute:
    needle = 'rename_all = "camelCase"'
    if needle not in attribute:
        raise SystemExit("StreemPilot RealtimeEvent camelCase attribute was not found")
    replacement = attribute[:-2] + ', rename_all_fields = "camelCase")]'
    text = text[:attribute_start] + replacement + text[attribute_end:]

path.write_text(text, encoding="utf-8")
PY

chmod 0755 \
  "$output/generate_streempilot_sp_fleet.py" \
  "$output/finalize_requested_product_fleets.py" \
  "$output/run_protected_requested_product_fleets_20260804.sh"
python3 -m py_compile \
  "$output/generate_streempilot_sp_fleet.py" \
  "$output/finalize_requested_product_fleets.py"
bash -n "$output/run_protected_requested_product_fleets_20260804.sh"

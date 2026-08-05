#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly REGION="${1:?AWS region is required}"
readonly ROOT="$(git rev-parse --show-toplevel)"
readonly SELECTOR="$ROOT/scripts/ops/select_hypesiege_github_app_from_protected_sources.py"
readonly PUBLISHER="$ROOT/scripts/ops/publish_hypesiege_hsg_fleet_with_app.sh"

work="$(mktemp -d /tmp/hsg-protected-runtime.XXXXXX)"
export HOME="$work/home"
export RUNNER_TEMP="$work"
mkdir -p "$HOME"
chmod 700 "$HOME"

cleanup() {
  unset K8S_SUBMODULE_APP_ID K8S_SUBMODULE_APP_PRIVATE_KEY GH_TOKEN
  python3 - "$work" <<'PY'
import shutil
import sys
from pathlib import Path
path = Path(sys.argv[1])
if path.exists():
    shutil.rmtree(path)
PY
}
trap cleanup EXIT

emit_base64() {
  local marker="$1"
  local path="$2"
  if [[ -s "$path" ]]; then
    printf '%s=' "$marker"
    base64 --wrap=0 "$path"
    printf '\n'
  fi
}

write_failure_diagnostic() {
  local output="$1"
  local stage="$2"
  local message="$3"
  local exit_code="$4"
  jq -n \
    --arg status failed \
    --arg stage "$stage" \
    --arg message "$message" \
    --argjson exit_code "$exit_code" \
    '{schema_version:1,status:$status,stage:$stage,message:$message,exit_code:$exit_code,pat_used:false}' \
    > "$output"
}

missing=()
for command in aws base64 curl git jq kubectl openssl python3 sha256sum tar; do
  command -v "$command" >/dev/null || missing+=("$command")
done
if (( ${#missing[@]} > 0 )); then
  diagnostic="$work/hsg-selector-diagnostic.json"
  write_failure_diagnostic \
    "$diagnostic" \
    prerequisites \
    "required commands unavailable: ${missing[*]}" \
    1
  emit_base64 HSG_SELECTOR_DIAGNOSTIC_BASE64 "$diagnostic"
  exit 1
fi

app_id_file="$work/github-app-id"
private_key_file="$work/github-app-private-key.pem"
selector_evidence="$work/hsg-protected-app-selector.json"
selector_log="$work/hsg-selector.log"

set +e
python3 "$SELECTOR" \
  --organization hypesiege \
  --region "$REGION" \
  --app-id-out "$app_id_file" \
  --private-key-out "$private_key_file" \
  --evidence-out "$selector_evidence" \
  > "$selector_log" 2>&1
selector_status=$?
set -e
cat "$selector_log"

if [[ "$selector_status" -ne 0 ]]; then
  diagnostic="$work/hsg-selector-diagnostic.json"
  selector_message="$(tail -n 1 "$selector_log" | tr -d '\r' | cut -c1-4000)"
  write_failure_diagnostic \
    "$diagnostic" \
    select-protected-app \
    "$selector_message" \
    "$selector_status"
  emit_base64 HSG_SELECTOR_DIAGNOSTIC_BASE64 "$diagnostic"
  exit "$selector_status"
fi

test -s "$app_id_file"
test -s "$private_key_file"
test -s "$selector_evidence"

export K8S_SUBMODULE_APP_ID="$(tr -d '\r\n' < "$app_id_file")"
export K8S_SUBMODULE_APP_PRIVATE_KEY="$(cat "$private_key_file")"
publisher_log="$work/hsg-publisher.log"
chmod 700 "$PUBLISHER"

set +e
"$PUBLISHER" > "$publisher_log" 2>&1
publisher_status=$?
set -e
cat "$publisher_log"
unset K8S_SUBMODULE_APP_ID K8S_SUBMODULE_APP_PRIVATE_KEY GH_TOKEN

emit_base64 HSG_SELECTOR_BASE64 "$selector_evidence"
emit_base64 \
  HSG_PUBLISHER_DIAGNOSTIC_BASE64 \
  "$work/hypesiege-hsg-publication-diagnostic.json"

if [[ "$publisher_status" -ne 0 ]]; then
  exit "$publisher_status"
fi

report="$work/hypesiege-hsg-publication.json"
test -s "$report"
emit_base64 HSG_REPORT_BASE64 "$report"

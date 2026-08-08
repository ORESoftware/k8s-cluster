#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

trusted_sha="${1:?trusted k8s-cluster SHA required}"
region="${2:?AWS region required}"
[[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]]

work="$(mktemp -d /tmp/den-896-streempilot-test-promote.XXXXXX)"
active_token=''

revoke_token() {
  if [[ -n "$active_token" ]]; then
    GH_TOKEN="$active_token" gh api --method DELETE /installation/token >/dev/null 2>&1 || true
    active_token=''
  fi
}

cleanup() {
  revoke_token
  unset GH_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN
  find "$work" -type f \( -name '*token*' -o -name '*.pem' \) -exec shred -u {} + 2>/dev/null || true
  rm -rf "$work"
}

report_failure() {
  local status=$?
  for log in "$work"/*.log; do
    [[ -f "$log" ]] || continue
    printf '\n===== %s =====\n' "$log" >&2
    tail -c 30000 "$log" >&2 || true
  done
  exit "$status"
}

trap cleanup EXIT
trap report_failure ERR

for command in aws base64 curl git jq kubectl mktemp openssl python3 sha256sum shred tar uname; do
  command -v "$command" >/dev/null || {
    printf 'missing required protected-host command: %s\n' "$command" >&2
    exit 70
  }
done

git init "$work/k8s-cluster" >/dev/null
git -C "$work/k8s-cluster" remote add origin https://github.com/ORESoftware/k8s-cluster.git
git -C "$work/k8s-cluster" fetch --quiet --depth=1 origin "$trusted_sha"
git -C "$work/k8s-cluster" switch --quiet --detach FETCH_HEAD
[[ "$(git -C "$work/k8s-cluster" rev-parse HEAD)" == "$trusted_sha" ]]

installer="$work/k8s-cluster/scripts/ops/install_pinned_github_cli.sh"
gh_binary="$(bash "$installer" --install-dir "$work/pinned-gh")"
[[ -x "$gh_binary" ]]
export PATH="$(dirname "$gh_binary"):$PATH"
[[ "$(command -v gh)" == "$gh_binary" ]]

selector="$work/k8s-cluster/scripts/ops/select_hypesiege_github_app_from_protected_sources.py"
token_helper="$work/k8s-cluster/scripts/ops/mint_repository_admin_app_token.py"
publisher="$work/k8s-cluster/scripts/ops/publish_streempilot_test_then_promote.py"
for path in "$selector" "$token_helper" "$publisher"; do
  [[ -f "$path" ]] || {
    printf 'missing trusted promotion component: %s\n' "$path" >&2
    exit 71
  }
done

run_target() {
  local organization="$1"
  local target="$2"
  local evidence="$3"
  local stage_evidence="${4:-}"
  local safe_name="${organization//[^A-Za-z0-9_.-]/_}"
  local token_file="$work/${safe_name}-installation-token"
  local selector_evidence="$work/${safe_name}-selector.json"
  local publication_log="$work/${safe_name}-${target}.log"

  python3 "$token_helper" \
    --selector "$selector" \
    --organization "$organization" \
    --token-out "$token_file" \
    --evidence-out "$selector_evidence" \
    --region "$region"

  [[ -s "$token_file" ]]
  active_token="$(tr -d '\r\n' < "$token_file")"
  [[ ${#active_token} -ge 20 ]]
  export GH_TOKEN="$active_token"
  export GITHUB_REPOSITORY_ADMIN_TOKEN="$active_token"

  args=(
    python3 "$publisher"
    --target "$target"
    --evidence-out "$evidence"
  )
  if [[ -n "$stage_evidence" ]]; then
    args+=(--stage-evidence "$stage_evidence")
  fi

  set +e
  "${args[@]}" >"$publication_log" 2>&1
  local status=$?
  set -e
  if [[ "$status" -ne 0 ]]; then
    tail -c 30000 "$publication_log" >&2 || true
    return "$status"
  fi

  grep -F "VERIFIED_STREEMPILOT_CANONICAL_GAPS target=$target" "$publication_log" >/dev/null
  [[ -s "$evidence" ]]

  revoke_token
  unset GH_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN
  shred -u "$token_file" 2>/dev/null || rm -f "$token_file"
}

stage_evidence="$work/streempilot-test-stage.json"
production_evidence="$work/streempilot-production.json"

# Hard promotion boundary: production token minting and mutation are unreachable
# until the test organization has the exact four private/main sealed histories.
run_target StreemPilot-test stage "$stage_evidence"
run_target StreemPilot production "$production_evidence" "$stage_evidence"

python3 - "$stage_evidence" "$production_evidence" "$work/combined-report.json" "$trusted_sha" <<'PY'
import json
import sys
from pathlib import Path

stage_path, production_path, output = map(Path, sys.argv[1:4])
trusted_sha = sys.argv[4]
stage = json.loads(stage_path.read_text(encoding="utf-8"))
production = json.loads(production_path.read_text(encoding="utf-8"))

expected = {
    "StreemPilot/streempilot-compositor.rs",
    "StreemPilot/streempilot-destinations",
    "StreemPilot/streempilot-recording.rs",
    "StreemPilot/streempilot-webrtc-adapter.rs",
}

def index(document, target, organization):
    if document.get("target") != target:
        raise SystemExit(f"{target} evidence target mismatch")
    if document.get("target_organization") != organization:
        raise SystemExit(f"{target} evidence organization mismatch")
    rows = document.get("repositories")
    if not isinstance(rows, list) or len(rows) != 4:
        raise SystemExit(f"{target} evidence repository count mismatch")
    result = {}
    for row in rows:
        canonical = row.get("canonical_full_name")
        if canonical not in expected or canonical in result:
            raise SystemExit(f"{target} canonical identity mismatch: {canonical!r}")
        if row.get("visibility") != "private" or row.get("default_branch") != "main":
            raise SystemExit(f"{target} repository state mismatch: {canonical}")
        if not isinstance(row.get("repository_id"), int) or row["repository_id"] <= 0:
            raise SystemExit(f"{target} repository id mismatch: {canonical}")
        main_sha = row.get("main_sha")
        expected_sha = row.get("expected_sealed_sha")
        if (
            not isinstance(main_sha, str)
            or len(main_sha) != 40
            or main_sha != expected_sha
        ):
            raise SystemExit(f"{target} sealed SHA mismatch: {canonical}")
        result[canonical] = row
    if set(result) != expected:
        raise SystemExit(f"{target} evidence does not cover exact canonical set")
    return result

stage_rows = index(stage, "stage", "StreemPilot-test")
production_rows = index(production, "production", "StreemPilot")
for canonical in sorted(expected):
    if stage_rows[canonical]["main_sha"] != production_rows[canonical]["main_sha"]:
        raise SystemExit(f"promotion SHA mismatch: {canonical}")

report = {
    "schema_version": 1,
    "trusted_k8s_cluster_sha": trusted_sha,
    "sealed_source_repository": stage["sealed_source_repository"],
    "sealed_source_sha": stage["sealed_source_sha"],
    "stage": stage,
    "production": production,
}
output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
PY

printf 'STREEMPILOT_PROMOTION_REPORT_BASE64='
base64 --wrap=0 "$work/combined-report.json"
printf '\nVERIFIED_STREEMPILOT_TEST_PROMOTION 4/4\n'

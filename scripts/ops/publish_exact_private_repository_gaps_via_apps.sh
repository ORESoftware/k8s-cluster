#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

trusted_sha="${1:?trusted k8s-cluster SHA required}"
region="${2:?AWS region required}"
[[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]]

work="$(mktemp -d /tmp/den-2328-protected-app-gaps.XXXXXX)"
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
publisher="$work/k8s-cluster/scripts/ops/publish_exact_private_repository_gaps.py"
for path in "$selector" "$token_helper" "$publisher"; do
  [[ -f "$path" ]] || {
    printf 'missing trusted publisher component: %s\n' "$path" >&2
    exit 71
  }
done

publish_organization() {
  local organization="$1"
  local token_file="$work/${organization}-installation-token"
  local selector_evidence="$work/${organization}-selector.json"
  local publication_evidence="$work/${organization}-publication.json"
  local publication_log="$work/${organization}-publication.log"

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

  set +e
  python3 "$publisher" \
    --organization "$organization" \
    --evidence-out "$publication_evidence" \
    >"$publication_log" 2>&1
  local status=$?
  set -e
  if [[ "$status" -ne 0 ]]; then
    tail -c 30000 "$publication_log" >&2 || true
    return "$status"
  fi

  grep -F 'VERIFIED_EXACT_PRIVATE_GAPS' "$publication_log" >/dev/null
  [[ -s "$publication_evidence" ]]

  revoke_token
  unset GH_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN
  shred -u "$token_file" 2>/dev/null || rm -f "$token_file"
}

publish_organization hypesiege
publish_organization StreemPilot

python3 - \
  "$work/hypesiege-selector.json" \
  "$work/hypesiege-publication.json" \
  "$work/StreemPilot-selector.json" \
  "$work/StreemPilot-publication.json" \
  "$work/combined-report.json" \
  "$trusted_sha" <<'PY'
import json
import sys
from pathlib import Path

h_selector, h_publication, s_selector, s_publication, output = map(
    Path, sys.argv[1:6]
)
trusted_sha = sys.argv[6]
h = json.loads(h_publication.read_text(encoding="utf-8"))
s = json.loads(s_publication.read_text(encoding="utf-8"))
repositories = h["repositories"] + s["repositories"]
expected = {
    "StreemPilot/streempilot-media-router.rs",
    "hypesiege/hypesiege-analytics.rs",
    "hypesiege/hypesiege-publishing-worker.rs",
    "hypesiege/hypesiege-scheduler.rs",
}
observed = {item["full_name"] for item in repositories}
if observed != expected:
    raise SystemExit(f"exact repository evidence mismatch: {sorted(observed)}")
for item in repositories:
    if item.get("visibility") != "private" or item.get("default_branch") != "main":
        raise SystemExit(f"invalid repository state: {item.get('full_name')}")
    if not isinstance(item.get("repository_id"), int) or item["repository_id"] <= 0:
        raise SystemExit(f"invalid repository id: {item.get('full_name')}")
    main_sha = item.get("main_sha")
    if not isinstance(main_sha, str) or len(main_sha) != 40:
        raise SystemExit(f"invalid main SHA: {item.get('full_name')}")

safe_selector_fields = (
    "app_slug",
    "installation_id",
    "repository_selection",
    "permissions",
)
selectors = []
for path in (h_selector, s_selector):
    document = json.loads(path.read_text(encoding="utf-8"))
    selectors.append({key: document.get(key) for key in safe_selector_fields})

report = {
    "schema_version": 1,
    "trusted_k8s_cluster_sha": trusted_sha,
    "repositories": sorted(repositories, key=lambda item: item["full_name"].casefold()),
    "selectors": selectors,
}
output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
PY

printf 'EXACT_GAPS_REPORT_BASE64='
base64 --wrap=0 "$work/combined-report.json"
printf '\nVERIFIED_PROTECTED_APP_EXACT_GAPS 4/4\n'

#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

trusted_sha="${1:?trusted k8s-cluster SHA required}"
region="${2:?AWS region required}"
[[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]]

work="$(mktemp -d /tmp/benefactor-service-app-publisher.XXXXXX)"
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
publisher="$work/k8s-cluster/scripts/ops/publish_benefactor_service_repositories.py"
for path in "$selector" "$token_helper" "$publisher"; do
  [[ -f "$path" ]] || {
    printf 'missing trusted publisher component: %s\n' "$path" >&2
    exit 71
  }
done

token_file="$work/benefactor-installation-token"
selector_evidence="$work/benefactor-selector.json"
publication_evidence="$work/benefactor-publication.json"
publication_log="$work/benefactor-publication.log"

python3 "$token_helper" \
  --selector "$selector" \
  --organization benefactor-cc \
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
  --trusted-k8s-cluster-sha "$trusted_sha" \
  --evidence-out "$publication_evidence" \
  >"$publication_log" 2>&1
status=$?
set -e
if [[ "$status" -ne 0 ]]; then
  tail -c 30000 "$publication_log" >&2 || true
  exit "$status"
fi

cat "$publication_log"
[[ -s "$publication_evidence" ]]
jq -e '
  .schema_version == 1 and
  .organization == "benefactor-cc" and
  (.trusted_k8s_cluster_sha | test("^[0-9a-f]{40}$")) and
  (.repositories | length == 3) and
  (([.repositories[].full_name] | sort) == ([
    "benefactor-cc/benefactor-api-server.rs",
    "benefactor-cc/benefactor-infra",
    "benefactor-cc/benefactor-web-server.rs"
  ] | sort)) and
  (all(.repositories[];
    .visibility == "private" and
    .default_branch == "main" and
    (.repository_id | type == "number" and . > 0) and
    (.main_sha | test("^[0-9a-f]{40}$"))))
' "$publication_evidence" >/dev/null

revoke_token
unset GH_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN
shred -u "$token_file" 2>/dev/null || rm -f "$token_file"
printf 'VERIFIED_BENEFACTOR_SERVICE_REPOSITORIES 3/3\n'

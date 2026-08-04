#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

remote_stage="bootstrap"
on_error() {
  local status=$?
  printf 'MCP_PUBLISHER_ERROR stage=%s code=%d\n' "$remote_stage" "$status"
  exit "$status"
}
cleanup() {
  unset GH_TOKEN encoded_pat
  rm -rf "${work:-}" /tmp/requested-mcp-publication.json
}
trap on_error ERR
trap cleanup EXIT

trusted_sha="${1:?trusted SHA required}"
[[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]]
work="$(mktemp -d /tmp/requested-mcp-publisher.XXXXXX)"

remote_stage="host-prerequisites"
command -v kubectl >/dev/null
command -v git >/dev/null
command -v python3 >/dev/null
test -r /etc/kubernetes/admin.conf

remote_stage="kubernetes-github-credential"
encoded_pat="$(
  KUBECONFIG=/etc/kubernetes/admin.conf \
    kubectl -n default get secret dd-agent-secrets \
    -o jsonpath='{.data.GH_PAT}'
)"
test -n "$encoded_pat"
GH_TOKEN="$(printf '%s' "$encoded_pat" | base64 --decode)"
unset encoded_pat
test -n "$GH_TOKEN"
[[ "$GH_TOKEN" != *$'\n'* && "$GH_TOKEN" != *$'\r'* ]]
export GH_TOKEN

remote_stage="trusted-source-checkout"
git init "$work/k8s-cluster" >/dev/null
git -C "$work/k8s-cluster" remote add origin https://github.com/ORESoftware/k8s-cluster.git
git -C "$work/k8s-cluster" fetch --quiet --depth=1 origin "$trusted_sha"
git -C "$work/k8s-cluster" checkout --quiet --detach FETCH_HEAD
test "$(git -C "$work/k8s-cluster" rev-parse HEAD)" = "$trusted_sha"

remote_stage="publisher-contract-tests"
cd "$work/k8s-cluster"
python3 -m py_compile \
  scripts/ops/publish_requested_mcp_servers.py \
  scripts/ops/requested_mcp_publisher/*.py
python3 -m unittest -v scripts/ops/tests/test_publish_requested_mcp_servers.py

remote_stage="github-preflight-and-publication"
python3 scripts/ops/publish_requested_mcp_servers.py \
  --execute \
  --report /tmp/requested-mcp-publication.json

remote_stage="publication-report-validation"
python3 - <<'PY'
import json
from pathlib import Path

report = json.loads(Path('/tmp/requested-mcp-publication.json').read_text())
rows = report.get('repositories')
if not isinstance(rows, list) or len(rows) != 5:
    raise SystemExit('publication report must contain exactly five repositories')
for row in rows:
    print(
        'MCP_REPOSITORY_VERIFIED '
        f"{row['full_name']} visibility={row['visibility']} "
        f"main={row['current_main_sha']}"
    )
PY

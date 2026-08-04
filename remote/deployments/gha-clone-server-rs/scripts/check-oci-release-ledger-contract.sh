#!/usr/bin/env bash
set -euo pipefail

workflow='.github/workflows/gha-continuity-images.yml'
documentation='docs/gha-continuity-images.md'
renderer='remote/deployments/gha-clone-server-rs/scripts/render-oci-release-ledger-entry.sh'
classifier='remote/deployments/gha-clone-server-rs/scripts/check-oci-release-ledger-comments.py'

require_literal() {
  local file="$1"
  local literal="$2"
  if ! grep -Fq -- "$literal" "$file"; then
    printf 'missing required OCI ledger contract in %s: %s\n' "$file" "$literal" >&2
    exit 1
  fi
}

for required in \
  'issues: write # Append validated immutable release metadata to issue 702.' \
  'OCI_RELEASE_LEDGER_ISSUE: "702"' \
  'render-oci-release-ledger-entry.sh' \
  'check-oci-release-ledger-comments.py' \
  'gh api --paginate --slurp' \
  'release marker already exists with conflicting metadata' \
  'github.event_name == '\''push'\''' \
  'github.event_name == '\''workflow_dispatch'\'''
do
  require_literal "$workflow" "$required"
done

for required in \
  'GHA continuity OCI release digest ledger' \
  'ORESoftware/k8s-cluster#702' \
  'image@sha256' \
  'workflow-scoped `GITHUB_TOKEN`' \
  'reproducibility conflict' \
  'ghcr.io/oresoftware/gha-capacity-broker'
do
  require_literal "$documentation" "$required"
done

for required in \
  "repository\" != 'ORESoftware/k8s-cluster'" \
  '[[ ! "$source_sha" =~ ^[0-9a-f]{40}$ ]]' \
  '[[ ! "$digest" =~ ^sha256:[0-9a-f]{64}$ ]]' \
  "expected_image='ghcr.io/oresoftware/gha-clone-server'" \
  "expected_image='ghcr.io/oresoftware/gha-executor-router'" \
  "expected_image='ghcr.io/oresoftware/gha-capacity-broker'" \
  'immutable_ref="${image}@${digest}"' \
  "printf '<!-- gha-continuity-oci-release:%s:%s -->\\n'" \
  '"schema_version":1'
do
  require_literal "$renderer" "$required"
done

for required in \
  'return 10' \
  'return 0' \
  'release marker already exists with conflicting metadata'
do
  require_literal "$classifier" "$required"
done

if [[ "$(grep -Ec '^[[:space:]]+issues:[[:space:]]+write' "$workflow")" -ne 1 ]]; then
  printf 'issue-write permission must occur exactly once\n' >&2
  exit 1
fi
if grep -Fq 'pull_request_target:' "$workflow"; then
  printf 'OCI publication workflow must not use pull_request_target\n' >&2
  exit 1
fi
if grep -Eq '\$\{\{[[:space:]]*secrets\.' "$workflow"; then
  printf 'OCI publication workflow must not depend on repository secrets\n' >&2
  exit 1
fi

bash -n "$renderer" "$0"
python3 -m py_compile "$classifier"

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

sample_sha='0123456789abcdef0123456789abcdef01234567'
sample_digest='sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef'
sample_image='ghcr.io/oresoftware/gha-executor-router'
sample_ref="${sample_image}@${sample_digest}"
entry="${tmpdir}/entry.md"
comments="${tmpdir}/comments.json"

bash "$renderer" \
  'ORESoftware/k8s-cluster' \
  "$sample_sha" \
  'executor-router' \
  "$sample_image" \
  "$sample_digest" >"$entry"

for exact in \
  "<!-- gha-continuity-oci-release:${sample_sha}:executor-router -->" \
  '```json' \
  "{\"schema_version\":1,\"repository\":\"ORESoftware/k8s-cluster\",\"source_sha\":\"${sample_sha}\",\"target\":\"executor-router\",\"image\":\"${sample_image}\",\"digest\":\"${sample_digest}\",\"ref\":\"${sample_ref}\"}" \
  '```'
do
  if ! grep -Fxq -- "$exact" "$entry"; then
    printf 'renderer output missing exact line: %s\n' "$exact" >&2
    exit 1
  fi
done

capacity_image='ghcr.io/oresoftware/gha-capacity-broker'
capacity_ref="${capacity_image}@${sample_digest}"
capacity_entry="${tmpdir}/capacity-entry.md"
bash "$renderer" \
  'ORESoftware/k8s-cluster' \
  "$sample_sha" \
  'capacity-broker' \
  "$capacity_image" \
  "$sample_digest" >"$capacity_entry"
for exact in \
  "<!-- gha-continuity-oci-release:${sample_sha}:capacity-broker -->" \
  "{\"schema_version\":1,\"repository\":\"ORESoftware/k8s-cluster\",\"source_sha\":\"${sample_sha}\",\"target\":\"capacity-broker\",\"image\":\"${capacity_image}\",\"digest\":\"${sample_digest}\",\"ref\":\"${capacity_ref}\"}"
do
  if ! grep -Fxq -- "$exact" "$capacity_entry"; then
    printf 'capacity renderer output missing exact line: %s\n' "$exact" >&2
    exit 1
  fi
done

expect_renderer_failure() {
  if bash "$renderer" "$@" >/dev/null 2>&1; then
    printf 'renderer unexpectedly accepted invalid release metadata\n' >&2
    exit 1
  fi
}

expect_renderer_failure \
  'Other/repository' "$sample_sha" 'executor-router' "$sample_image" "$sample_digest"
expect_renderer_failure \
  'ORESoftware/k8s-cluster' 'not-a-commit' 'executor-router' "$sample_image" "$sample_digest"
expect_renderer_failure \
  'ORESoftware/k8s-cluster' "$sample_sha" 'unexpected-target' "$sample_image" "$sample_digest"
expect_renderer_failure \
  'ORESoftware/k8s-cluster' "$sample_sha" 'executor-router' \
  'ghcr.io/oresoftware/gha-clone-server' "$sample_digest"
expect_renderer_failure \
  'ORESoftware/k8s-cluster' "$sample_sha" 'capacity-broker' \
  'ghcr.io/oresoftware/gha-executor-router' "$sample_digest"
expect_renderer_failure \
  'ORESoftware/k8s-cluster' "$sample_sha" 'executor-router' "$sample_image" 'sha256:ABCDEF'

printf '[]\n' >"$comments"
set +e
python3 "$classifier" "$entry" "$comments" >/dev/null
status=$?
set -e
if [[ "$status" -ne 10 ]]; then
  printf 'absent marker must return status 10, got %s\n' "$status" >&2
  exit 1
fi

python3 - "$entry" "$comments" <<'PY'
import json
import sys
from pathlib import Path
body = Path(sys.argv[1]).read_text()
Path(sys.argv[2]).write_text(json.dumps([[{"id": 1, "body": body}]]))
PY
python3 "$classifier" "$entry" "$comments" | grep -Fxq 'present'

python3 - "$entry" "$comments" <<'PY'
import json
import sys
from pathlib import Path
body = Path(sys.argv[1]).read_text().replace('"digest":"sha256:', '"digest":"sha256:f')
Path(sys.argv[2]).write_text(json.dumps([[{"id": 2, "body": body}]]))
PY
if python3 "$classifier" "$entry" "$comments" >/dev/null 2>&1; then
  printf 'conflicting marker unexpectedly passed\n' >&2
  exit 1
fi

printf 'OCI release digest ledger contract passed\n'

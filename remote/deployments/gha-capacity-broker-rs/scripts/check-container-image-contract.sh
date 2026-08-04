#!/usr/bin/env bash
set -euo pipefail

workflow='.github/workflows/gha-capacity-broker-image.yml'
dockerfile='remote/deployments/gha-capacity-broker-rs/Dockerfile'
smoke='remote/deployments/gha-capacity-broker-rs/scripts/smoke-container-image.sh'
validator='remote/deployments/gha-clone-server-rs/scripts/validate-image-export.py'
renderer='remote/deployments/gha-clone-server-rs/scripts/render-oci-release-ledger-entry.sh'
documentation='docs/gha-continuity-images.md'

require_literal() {
  local file="$1"
  local literal="$2"
  if ! grep -Fq -- "$literal" "$file"; then
    printf 'missing capacity-broker image contract in %s: %s\n' "$file" "$literal" >&2
    exit 1
  fi
}

for required in \
  'ARG RUST_IMAGE=docker.io/library/rust:1.90.0-bookworm@sha256:3914072ca0c3b8aad871db9169a651ccfce30cf58303e5d6f2db16d1d8a7e58f' \
  'ARG RUNTIME_IMAGE=docker.io/library/debian:bookworm-slim@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818' \
  'COPY Cargo.toml Cargo.lock ./' \
  'cargo build --locked --release --bin gha-capacity-broker' \
  'org.opencontainers.image.revision="${OCI_REVISION}"' \
  'org.opencontainers.image.source="${OCI_SOURCE}"' \
  'USER 65532:65532' \
  'ENTRYPOINT ["/usr/local/bin/gha-capacity-broker"]'
do
  require_literal "$dockerfile" "$required"
done

for required in \
  'permissions:' \
  'contents: read' \
  'packages: write' \
  'issues: write # Append validated immutable release metadata to issue 702.' \
  'ghcr.io/oresoftware/gha-capacity-broker' \
  'TARGET: capacity-broker' \
  'OCI_RELEASE_LEDGER_ISSUE: "702"' \
  'render-oci-release-ledger-entry.sh' \
  'check-oci-release-ledger-comments.py' \
  '--sbom=true' \
  '--provenance=mode=max,version=v1' \
  'aquasecurity/trivy-action@57a97c7e7821a5776cebc9bb87c984fa69cba8f1' \
  "github.event_name == 'push'" \
  "github.event_name == 'workflow_dispatch'"
do
  require_literal "$workflow" "$required"
done

for required in \
  '--read-only' \
  '--security-opt no-new-privileges:true' \
  '--cap-drop ALL' \
  'GHA_MUTATION_ENABLED=false' \
  'GITHUB_MUTATION_APP_ID=1001' \
  'GITHUB_BILLING_APP_ID=1002' \
  '/healthz' \
  '/readyz' \
  '/api/v1/capabilities' \
  'githubTokenRequested":false'
do
  require_literal "$smoke" "$required"
done

require_literal "$validator" '"gha-capacity-broker"'
require_literal "$renderer" "expected_image='ghcr.io/oresoftware/gha-capacity-broker'"
require_literal "$documentation" '`gha-capacity-broker`'
require_literal "$documentation" 'ghcr.io/oresoftware/gha-capacity-broker'

if [[ "$(grep -Ec '^[[:space:]]+issues:[[:space:]]+write' "$workflow")" -ne 1 ]]; then
  printf 'capacity-broker issue-write permission must occur exactly once\n' >&2
  exit 1
fi
if grep -Fq 'pull_request_target:' "$workflow"; then
  printf 'capacity-broker publication workflow must not use pull_request_target\n' >&2
  exit 1
fi
if grep -Eq '\$\{\{[[:space:]]*secrets\.' "$workflow"; then
  printf 'capacity-broker publication workflow must not depend on repository secrets\n' >&2
  exit 1
fi
classic_pat_prefix='gh''p_'
fine_grained_pat_prefix='github''_pat_'
if grep -Eq "(^|[[:space:]])(${classic_pat_prefix}|${fine_grained_pat_prefix})" \
  "$workflow" "$dockerfile" "$smoke"; then
  printf 'credential marker found in capacity-broker image files\n' >&2
  exit 1
fi

bash -n "$smoke" "$renderer" "$0"
python3 -m py_compile "$validator"

sample_sha='0123456789abcdef0123456789abcdef01234567'
sample_digest='sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789'
sample_image='ghcr.io/oresoftware/gha-capacity-broker'
entry="$(mktemp)"
trap 'rm -f "$entry"' EXIT

bash "$renderer" \
  'ORESoftware/k8s-cluster' \
  "$sample_sha" \
  'capacity-broker' \
  "$sample_image" \
  "$sample_digest" >"$entry"

grep -Fxq "<!-- gha-continuity-oci-release:${sample_sha}:capacity-broker -->" "$entry"
grep -Fq '"target":"capacity-broker"' "$entry"
grep -Fq '"ref":"ghcr.io/oresoftware/gha-capacity-broker@sha256:' "$entry"

if bash "$renderer" \
  'ORESoftware/k8s-cluster' \
  "$sample_sha" \
  'capacity-broker' \
  'ghcr.io/oresoftware/gha-executor-router' \
  "$sample_digest" >/dev/null 2>&1; then
  printf 'capacity-broker renderer accepted a mismatched image\n' >&2
  exit 1
fi

printf 'capacity-broker container image contract passed\n'

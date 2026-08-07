#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

trusted_sha="${1:?trusted k8s-cluster SHA required}"
source_root="${2:?trusted source root required}"
[[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]]
[[ "$source_root" == /* ]]
[[ -d "$source_root" ]]

publisher_region="${AWS_REGION:-${AWS_DEFAULT_REGION:-us-east-1}}"
[[ "$publisher_region" =~ ^[a-z]{2}(-gov)?-[a-z0-9-]+-[0-9]$ ]]
runtime_dir="$(mktemp -d /tmp/test-org-repository-fleet.XXXXXX)"
chmod 700 "$runtime_dir"

cleanup() {
  unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN raw_pat secret_json
  rm -rf "$runtime_dir"
}
trap cleanup EXIT

valid_token() {
  local candidate="${1:-}"
  [[ -n "$candidate" ]] &&
    [[ "$candidate" != *$'\n'* ]] &&
    [[ "$candidate" != *$'\r'* ]] &&
    [[ "$candidate" != *$'\t'* ]] &&
    [[ "$candidate" != *' '* ]]
}

stage=credential
command -v aws >/dev/null 2>&1 || {
  printf 'publisher-stage=%s status=failed reason=aws-cli-unavailable\n' "$stage" >&2
  exit 69
}
secret_json="$(
  aws secretsmanager get-secret-value \
    --region "$publisher_region" \
    --secret-id dd/remote-dev/agent-secrets \
    --query SecretString \
    --output text
)"
raw_pat="$(
  printf '%s' "$secret_json" | python3 -c '
import json
import sys
payload = json.load(sys.stdin)
value = payload.get("GH_PAT")
if not isinstance(value, str) or not value or any(ch.isspace() for ch in value):
    raise SystemExit(65)
sys.stdout.write(value)
'
)"
unset secret_json
valid_token "$raw_pat" || {
  printf 'publisher-stage=%s status=failed reason=invalid-protected-github-token\n' "$stage" >&2
  exit 65
}
export GH_TOKEN="$raw_pat"
export GITHUB_REPOSITORY_ADMIN_TOKEN="$raw_pat"
unset raw_pat
printf 'publisher-stage=%s status=passed source=aws-secrets-manager\n' "$stage" >&2

stage=trusted-source
required_paths=(
  config/test_org_repository_fleets/index.json
  scripts/ops/bootstrap_test_org_repository_fleets.py
  scripts/ops/run_protected_test_org_repository_fleet_publisher.sh
  tests/ops/test_bootstrap_test_org_repository_fleets.py
)
for relative_path in "${required_paths[@]}"; do
  [[ -f "$source_root/$relative_path" ]] || {
    printf 'publisher-stage=%s status=failed reason=missing-path path=%s\n' "$stage" "$relative_path" >&2
    exit 66
  }
done
printf 'publisher-stage=%s status=passed sha=%s\n' "$stage" "$trusted_sha" >&2

stage=validation
bash -n "$source_root/scripts/ops/run_protected_test_org_repository_fleet_publisher.sh"
python3 -m py_compile "$source_root/scripts/ops/bootstrap_test_org_repository_fleets.py"
python3 -m unittest discover \
  -s "$source_root/tests/ops" \
  -p 'test_bootstrap_test_org_repository_fleets.py' \
  -v >&2
printf 'publisher-stage=%s status=passed\n' "$stage" >&2

stage=bounded-publication
json_report="$runtime_dir/test-org-repository-fleet.json"
markdown_report="$runtime_dir/test-org-repository-fleet.md"
publisher_log="$runtime_dir/test-org-repository-fleet.log"

set +e
python3 "$source_root/scripts/ops/bootstrap_test_org_repository_fleets.py" \
  --config "$source_root/config/test_org_repository_fleets/index.json" \
  --execute \
  --max-workers 3 \
  --json-report "$json_report" \
  --markdown-report "$markdown_report" \
  >"$publisher_log" 2>&1
publisher_status=$?
set -e

if [[ "$publisher_status" -ne 0 ]]; then
  LOG="$publisher_log" python3 - <<'PY_REDACT' >&2
import os
import re
from pathlib import Path
path = Path(os.environ["LOG"])
text = path.read_text(errors="replace") if path.exists() else "publisher log was not created"
patterns = [
    (r"https://x-access-token:[^@\s]+@github\.com/", "https://x-access-token:***@github.com/"),
    (r"github_pat_[A-Za-z0-9_]{20,}", "github_pat_***"),
    (r"gh[pousr]_[A-Za-z0-9_]{20,}", "gh*_***"),
    (r"(Authorization:\s*Bearer\s+)[A-Za-z0-9._-]{20,}", r"\1***"),
]
for pattern, replacement in patterns:
    text = re.sub(pattern, replacement, text, flags=re.I)
print("\n".join(text.splitlines()[-300:])[-50000:])
PY_REDACT
  printf 'publisher-stage=%s status=failed rc=%s\n' "$stage" "$publisher_status" >&2
  exit "$publisher_status"
fi

REPORT="$json_report" python3 - <<'PY_VERIFY'
import json
import os
from pathlib import Path
payload = json.loads(Path(os.environ["REPORT"]).read_text(encoding="utf-8"))
expected = {
    "mode": "execute",
    "organization_count": 18,
    "test_repository_count": 209,
    "dotgithub_repository_count": 18,
    "total_repository_count": 227,
}
for key, value in expected.items():
    if payload.get(key) != value:
        raise SystemExit(f"publisher report mismatch for {key}: {payload.get(key)!r} != {value!r}")
repositories = payload.get("repositories")
if not isinstance(repositories, list) or len(repositories) != 227:
    raise SystemExit("publisher report does not contain exactly 227 repository results")
seen = set()
for item in repositories:
    repository = item.get("repository")
    if not isinstance(repository, str) or repository.lower() in seen:
        raise SystemExit(f"invalid or duplicate repository report entry: {item!r}")
    seen.add(repository.lower())
    if item.get("error") or item.get("verified") is not True:
        raise SystemExit(f"unverified repository result: {item!r}")
    if item.get("action") not in {
        "already-current", "created-and-merged", "updated-and-merged",
        "created-pr-open", "updated-pr-open",
    }:
        raise SystemExit(f"unexpected repository action: {item!r}")
PY_VERIFY

[[ -s "$markdown_report" ]]
grep -Fq '<!-- test-org-repository-fleet-report-complete -->' "$markdown_report"
printf 'publisher-stage=%s status=passed\n' "$stage" >&2
cat "$markdown_report"

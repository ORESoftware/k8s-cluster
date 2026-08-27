#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

stage=all-protected-bootstrap

cleanup_environment() {
  unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN
  unset secret_json raw_pat parse_status
}
trap cleanup_environment EXIT
trap 'status=$?; printf "publisher-stage=%s status=failed rc=%s line=%s\n" "${stage:-all-protected-startup}" "$status" "$LINENO" >&2; exit "$status"' ERR

fail() {
  local reason="${1:?failure reason required}"
  local status="${2:-64}"
  printf 'publisher-stage=%s status=failed reason=%s rc=%s\n' \
    "$stage" "$reason" "$status" >&2
  exit "$status"
}

valid_token() {
  local candidate="${1:-}"
  test -n "$candidate" &&
    [[ "$candidate" != *$'\n'* ]] &&
    [[ "$candidate" != *$'\r'* ]] &&
    [[ "$candidate" != *$'\t'* ]] &&
    [[ "$candidate" != *' '* ]]
}

trusted_sha="${1:-}"
source_root="${2:-}"
[[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]] || fail invalid-trusted-sha 64
[[ "$source_root" == /* ]] || fail source-root-not-absolute 64
[[ -d "$source_root" ]] || fail source-root-missing 66

raw_region="${AWS_REGION:-${AWS_DEFAULT_REGION:-us-east-1}}"
command -v tr >/dev/null 2>&1 || fail tr-unavailable 69
publisher_region="$(printf '%s' "$raw_region" | tr -d '[:space:]')"
unset raw_region
[[ "$publisher_region" =~ ^[a-z]{2}(-gov)?-[a-z0-9-]+-[0-9]$ ]] || \
  fail invalid-aws-region 64
export AWS_REGION="$publisher_region"
export AWS_DEFAULT_REGION="$publisher_region"
unset publisher_region

required_paths=(
  scripts/ops/bootstrap_org_dotgithub_repositories.py
  scripts/ops/bootstrap_org_dotgithub_repositories_hardened.py
  scripts/ops/bootstrap_org_dotgithub_repositories_all.py
  tests/ops/test_bootstrap_org_dotgithub_repositories.py
  tests/ops/test_bootstrap_org_dotgithub_repositories_hardened.py
  tests/ops/test_bootstrap_org_dotgithub_repositories_all.py
)
for relative_path in "${required_paths[@]}"; do
  [[ -f "$source_root/$relative_path" ]] || fail trusted-source-missing 66
done
command -v aws >/dev/null 2>&1 || fail aws-unavailable 69
command -v python3 >/dev/null 2>&1 || fail python3-unavailable 69
printf 'publisher-stage=%s status=passed sha=%s\n' "$stage" "$trusted_sha" >&2

stage=all-protected-validation
python3 -m py_compile \
  "$source_root/scripts/ops/bootstrap_org_dotgithub_repositories.py" \
  "$source_root/scripts/ops/bootstrap_org_dotgithub_repositories_hardened.py" \
  "$source_root/scripts/ops/bootstrap_org_dotgithub_repositories_all.py"
python3 -m unittest discover \
  -s "$source_root/tests/ops" \
  -p 'test_bootstrap_org_dotgithub_repositories*.py' \
  -v >&2
printf 'publisher-stage=%s status=passed\n' "$stage" >&2

stage=all-protected-credential
if ! secret_json="$(
  aws secretsmanager get-secret-value \
    --region "$AWS_REGION" \
    --secret-id dd/remote-dev/agent-secrets \
    --query SecretString \
    --output text 2>/dev/null
)"; then
  fail protected-secret-unavailable 65
fi
[[ -n "$secret_json" ]] || fail protected-secret-empty 65

set +e
raw_pat="$(
  printf '%s' "$secret_json" | python3 -c '
import json
import sys
try:
    payload = json.load(sys.stdin)
except (json.JSONDecodeError, OSError):
    raise SystemExit(65)
value = payload.get("GH_PAT")
if not isinstance(value, str) or not value or any(ch.isspace() for ch in value):
    raise SystemExit(65)
sys.stdout.write(value)
' 2>/dev/null
)"
parse_status=$?
set -e
unset secret_json
[[ "$parse_status" -eq 0 ]] || fail protected-secret-invalid 65
valid_token "$raw_pat" || fail protected-token-invalid 65
GH_TOKEN="$raw_pat"
unset raw_pat parse_status
export GH_TOKEN
export GITHUB_REPOSITORY_ADMIN_TOKEN="$GH_TOKEN"
printf 'publisher-stage=%s status=passed source=aws-secrets-manager\n' "$stage" >&2

stage=all-protected-publication
runtime_dir="$(mktemp -d /tmp/org-dotgithub-all-governance.XXXXXX)"
chmod 700 "$runtime_dir"
json_report="$runtime_dir/org-dotgithub-governance.json"
markdown_report="$runtime_dir/org-dotgithub-governance.md"
publisher_log="$runtime_dir/org-dotgithub-governance.log"

set +e
python3 "$source_root/scripts/ops/bootstrap_org_dotgithub_repositories_all.py" \
  --execute \
  --json-report "$json_report" \
  --markdown-report "$markdown_report" \
  >"$publisher_log" 2>&1
publisher_status=$?
set -e

if [[ "$publisher_status" -ne 0 ]]; then
  cleanup_environment
  LOG="$publisher_log" python3 - <<'PY' >&2
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
print("\n".join(text.splitlines()[-220:])[-24000:])
PY
  printf 'publisher-stage=%s status=failed rc=%s\n' "$stage" "$publisher_status" >&2
  exit "$publisher_status"
fi
printf 'publisher-stage=%s status=passed\n' "$stage" >&2

stage=all-protected-verification
SOURCE_ROOT="$source_root" REPORT="$json_report" python3 - <<'PY'
import json
import os
from pathlib import Path
import sys

ops = Path(os.environ["SOURCE_ROOT"]) / "scripts" / "ops"
sys.path.insert(0, str(ops))
import bootstrap_org_dotgithub_repositories_all as publisher

payload = json.loads(Path(os.environ["REPORT"]).read_text(encoding="utf-8"))
organizations = payload.get("organizations")
expected = {name.lower() for name in publisher.TARGET_ORGANIZATIONS}
if payload.get("mode") != "execute":
    raise SystemExit("publisher report is not an execute report")
if not isinstance(organizations, list) or len(organizations) != 61:
    raise SystemExit("publisher report does not certify the fixed 61-organization fleet")
if expected & publisher.EXCLUDED_ORGANIZATIONS:
    raise SystemExit("excluded organizations entered the expected publication set")

seen = set()
for item in organizations:
    organization = item.get("organization")
    repository = item.get("repository")
    if not isinstance(organization, str) or repository != f"{organization}/.github":
        raise SystemExit(f"invalid organization report entry: {item!r}")
    lowered = organization.lower()
    if lowered in seen or item.get("verified") is not True:
        raise SystemExit(f"unverified or duplicate organization report entry: {item!r}")
    seen.add(lowered)
if seen != expected:
    raise SystemExit(
        f"organization report mismatch: missing={sorted(expected - seen)}, "
        f"extra={sorted(seen - expected)}"
    )
PY

test -s "$markdown_report"
cleanup_environment
printf 'publisher-stage=%s status=passed organizations=61\n' "$stage" >&2
cat "$markdown_report"
printf '\n<!-- org-dotgithub-governance-report-complete -->\n'

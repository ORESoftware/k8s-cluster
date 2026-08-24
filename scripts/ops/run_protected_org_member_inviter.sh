#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

stage=protected-bootstrap

cleanup_environment() {
  unset GH_TOKEN GITHUB_TOKEN
  unset secret_json raw_pat parse_status
}
trap cleanup_environment EXIT
trap 'status=$?; printf "inviter-stage=%s status=failed rc=%s line=%s\n" "${stage:-protected-startup}" "$status" "$LINENO" >&2; exit "$status"' ERR

fail() {
  local reason="${1:?failure reason required}"
  local status="${2:-64}"
  printf 'inviter-stage=%s status=failed reason=%s rc=%s\n' "$stage" "$reason" "$status" >&2
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

valid_username() {
  local candidate="${1:-}"
  [[ "$candidate" =~ ^[A-Za-z0-9]([A-Za-z0-9-]{0,37}[A-Za-z0-9])?$ ]] &&
    [[ "$candidate" != *--* ]]
}

trusted_sha="${1:-}"
source_root="${2:-}"
target_username="${3:-}"
[[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]] || fail invalid-trusted-sha 64
[[ "$source_root" == /* ]] || fail source-root-not-absolute 64
[[ -d "$source_root" ]] || fail source-root-missing 66
valid_username "$target_username" || fail invalid-target-username 64

raw_region="${AWS_REGION:-${AWS_DEFAULT_REGION:-us-east-1}}"
command -v tr >/dev/null 2>&1 || fail tr-unavailable 69
inviter_region="$(printf '%s' "$raw_region" | tr -d '[:space:]')"
unset raw_region
[[ "$inviter_region" =~ ^[a-z]{2}(-gov)?-[a-z0-9-]+-[0-9]$ ]] || fail invalid-aws-region 64
export AWS_REGION="$inviter_region"
export AWS_DEFAULT_REGION="$inviter_region"
unset inviter_region

required_paths=(
  scripts/ops/invite_org_member_all.py
  tests/ops/test_invite_org_member_all.py
  tests/ops/test_run_protected_org_member_inviter.py
)
for relative_path in "${required_paths[@]}"; do
  [[ -f "$source_root/$relative_path" ]] || fail trusted-source-missing 66
done
command -v aws >/dev/null 2>&1 || fail aws-unavailable 69
command -v python3 >/dev/null 2>&1 || fail python3-unavailable 69
printf 'inviter-stage=%s status=passed sha=%s target=%s\n' "$stage" "$trusted_sha" "$target_username" >&2

stage=protected-validation
python3 -m py_compile "$source_root/scripts/ops/invite_org_member_all.py"
python3 -m unittest discover \
  -s "$source_root/tests/ops" \
  -p 'test_*org_member_inviter*.py' \
  -v >&2
python3 -m unittest discover \
  -s "$source_root/tests/ops" \
  -p 'test_invite_org_member_all.py' \
  -v >&2
printf 'inviter-stage=%s status=passed\n' "$stage" >&2

stage=protected-credential
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
printf 'inviter-stage=%s status=passed source=aws-secrets-manager\n' "$stage" >&2

stage=protected-execution
runtime_dir="$(mktemp -d /tmp/org-member-inviter.XXXXXX)"
chmod 700 "$runtime_dir"
json_report="$runtime_dir/org-member-invitation.json"
markdown_report="$runtime_dir/org-member-invitation.md"
inviter_log="$runtime_dir/org-member-invitation.log"

set +e
python3 "$source_root/scripts/ops/invite_org_member_all.py" \
  --execute \
  --username "$target_username" \
  --expected-authenticated-login ORESoftware \
  --json-report "$json_report" \
  --markdown-report "$markdown_report" \
  >"$inviter_log" 2>&1
inviter_status=$?
set -e

if [[ "$inviter_status" -ne 0 ]]; then
  cleanup_environment
  LOG="$inviter_log" python3 - <<'PY' >&2
import os
import re
from pathlib import Path

path = Path(os.environ["LOG"])
text = path.read_text(errors="replace") if path.exists() else "inviter log was not created"
patterns = [
    (r"github_pat_[A-Za-z0-9_]{20,}", "github_pat_***"),
    (r"gh[pousr]_[A-Za-z0-9_]{20,}", "gh*_***"),
    (r"(Authorization:\s*Bearer\s+)[A-Za-z0-9._-]{20,}", r"\1***"),
]
for pattern, replacement in patterns:
    text = re.sub(pattern, replacement, text, flags=re.I)
print("\n".join(text.splitlines()[-220:])[-24000:])
PY
  if [[ -s "$markdown_report" ]]; then
    cat "$markdown_report"
  fi
  printf 'inviter-stage=%s status=failed rc=%s\n' "$stage" "$inviter_status" >&2
  exit "$inviter_status"
fi
printf 'inviter-stage=%s status=passed\n' "$stage" >&2

stage=protected-verification
REPORT="$json_report" TARGET_USERNAME="$target_username" python3 - <<'PY'
import json
import os
from pathlib import Path

payload = json.loads(Path(os.environ["REPORT"]).read_text(encoding="utf-8"))
if payload.get("mode") != "execute":
    raise SystemExit("invitation report is not an execute report")
if str(payload.get("authenticated_login", "")).lower() != "oresoftware":
    raise SystemExit("invitation report was not authenticated as ORESoftware")
if str(payload.get("target_username", "")).lower() != os.environ["TARGET_USERNAME"].lower():
    raise SystemExit("invitation report target mismatch")
organizations = payload.get("organizations")
if not isinstance(organizations, list) or not organizations:
    raise SystemExit("invitation report does not include owner organizations")
if payload.get("owner_organizations") != len(organizations):
    raise SystemExit("invitation report owner count mismatch")
if payload.get("counts", {}).get("failed", 0):
    raise SystemExit("one or more organization invitations failed")
allowed = {"already_member", "already_invited", "invited", "added"}
for item in organizations:
    if item.get("result") not in allowed:
        raise SystemExit(f"unexpected result: {item!r}")
PY

test -s "$markdown_report"
cleanup_environment
printf 'inviter-stage=%s status=passed organizations=%s target=%s\n' \
  "$stage" \
  "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["owner_organizations"])' "$json_report")" \
  "$target_username" >&2
cat "$markdown_report"

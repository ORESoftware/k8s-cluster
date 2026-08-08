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
runtime_dir="$(mktemp -d /tmp/org-dotgithub-governance-runtime.XXXXXX)"
chmod 700 "$runtime_dir"

cleanup_environment() {
  unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN
  unset raw_pat secret_json encoded_pat credential_source ec2_home
  unset profile_path profile_owner profile_mode profile_expected_uid profile_record
}
trap cleanup_environment EXIT

valid_token() {
  local candidate="${1:-}"
  test -n "$candidate" &&
    [[ "$candidate" != *$'\n'* ]] &&
    [[ "$candidate" != *$'\r'* ]] &&
    [[ "$candidate" != *$'\t'* ]] &&
    [[ "$candidate" != *' '* ]]
}

stage=protected-credential
GH_TOKEN=''
credential_source=''

# Prefer the protected host's instance role. The credential never crosses the
# GitHub-hosted runner or appears in SSM command arguments.
if command -v aws >/dev/null 2>&1; then
  secret_json="$(
    aws secretsmanager get-secret-value \
      --region "$publisher_region" \
      --secret-id dd/remote-dev/agent-secrets \
      --query SecretString \
      --output text 2>/dev/null || true
  )"
  if test -n "$secret_json"; then
    raw_pat="$(
      printf '%s' "$secret_json" | python3 -c '
import json
import sys
try:
    payload = json.load(sys.stdin)
except (json.JSONDecodeError, OSError):
    raise SystemExit(0)
value = payload.get("GH_PAT")
if isinstance(value, str) and value and not any(ch.isspace() for ch in value):
    sys.stdout.write(value)
' 2>/dev/null || true
    )"
    if valid_token "$raw_pat"; then
      GH_TOKEN="$raw_pat"
      credential_source=aws-secrets-manager
    fi
  fi
fi
unset raw_pat secret_json

# Fall back to the External-Secrets-reconciled Kubernetes Secret on the
# protected host. Only fixed kubeconfig paths and the fixed secret/key are read.
if test -z "$GH_TOKEN" && command -v kubectl >/dev/null 2>&1; then
  for kubeconfig in \
    /etc/kubernetes/admin.conf \
    /root/.kube/config \
    /home/ec2-user/.kube/config
  do
    test -r "$kubeconfig" || continue
    encoded_pat="$(
      KUBECONFIG="$kubeconfig" \
        kubectl -n default get secret dd-agent-secrets \
        -o jsonpath='{.data.GH_PAT}' 2>/dev/null || true
    )"
    test -n "$encoded_pat" || continue
    raw_pat="$(
      printf '%s' "$encoded_pat" | python3 -c '
import base64
import sys
try:
    value = base64.b64decode(sys.stdin.buffer.read(), validate=True).decode("utf-8")
except Exception:
    raise SystemExit(0)
if value and not any(ch.isspace() for ch in value):
    sys.stdout.write(value)
' 2>/dev/null || true
    )"
    if valid_token "$raw_pat"; then
      GH_TOKEN="$raw_pat"
      credential_source="kubernetes-secret:${kubeconfig}"
      break
    fi
  done
fi
unset raw_pat encoded_pat

# Prefer the protected ec2-user GitHub CLI abstraction when available.
if test -z "$GH_TOKEN" && \
   command -v sudo >/dev/null 2>&1 && \
   command -v getent >/dev/null 2>&1; then
  ec2_home="$(getent passwd ec2-user 2>/dev/null | awk -F: '$1 == "ec2-user" { print $6; exit }')"
  case "$ec2_home" in
    /*)
      raw_pat="$(
        sudo -u ec2-user -H \
          env \
            -u GH_TOKEN \
            -u GITHUB_TOKEN \
            -u GH_ENTERPRISE_TOKEN \
            -u GITHUB_REPOSITORY_ADMIN_TOKEN \
            -u GH_CONFIG_DIR \
            HOME="$ec2_home" \
            XDG_CONFIG_HOME="$ec2_home/.config" \
            bash --noprofile --norc -c \
              'command -v gh >/dev/null 2>&1 && gh auth token --hostname github.com' \
          2>/dev/null || true
      )"
      ;;
    *) raw_pat='' ;;
  esac
  if valid_token "$raw_pat"; then
    GH_TOKEN="$raw_pat"
    credential_source=protected-gh-profile
  fi
fi
unset raw_pat ec2_home

# A host may retain a file-backed gh profile after the binary is removed.
# Read only canonical paths, reject symlinks and unsafe ownership/modes, and
# parse the github.com oauth_token without printing it.
if test -z "$GH_TOKEN" && \
   command -v python3 >/dev/null 2>&1 && \
   command -v stat >/dev/null 2>&1; then
  ec2_uid="$(id -u ec2-user 2>/dev/null || true)"
  for profile_record in \
    '/root/.config/gh/hosts.yml:0' \
    "/home/ec2-user/.config/gh/hosts.yml:${ec2_uid}"
  do
    profile_path="${profile_record%:*}"
    profile_expected_uid="${profile_record##*:}"
    test -n "$profile_expected_uid" || continue
    test -f "$profile_path" || continue
    test ! -L "$profile_path" || continue
    test -r "$profile_path" || continue

    profile_owner="$(stat -c '%u' "$profile_path" 2>/dev/null || true)"
    profile_mode="$(stat -c '%a' "$profile_path" 2>/dev/null || true)"
    test "$profile_owner" = "$profile_expected_uid" || continue
    [[ "$profile_mode" =~ ^[0-7]{3,4}$ ]] || continue
    (( (8#$profile_mode & 0022) == 0 )) || continue

    raw_pat="$(
      GH_HOSTS_PROFILE="$profile_path" python3 - <<'PY' 2>/dev/null || true
import ast
import os
import re
import sys
from pathlib import Path

path = Path(os.environ["GH_HOSTS_PROFILE"])
current_host = None
for raw in path.read_text(encoding="utf-8").splitlines():
    stripped = raw.strip()
    if not stripped or stripped.startswith("#"):
        continue
    indent = len(raw) - len(raw.lstrip(" "))
    if indent == 0 and stripped.endswith(":"):
        current_host = stripped[:-1].strip().strip("'\"")
        continue
    if current_host != "github.com":
        continue
    match = re.match(r"^\s+oauth_token:\s*(.*?)\s*$", raw)
    if match is None:
        continue
    value = match.group(1)
    if len(value) >= 2 and value[0] == value[-1] and value[0] in "'\"":
        try:
            value = ast.literal_eval(value)
        except (SyntaxError, ValueError):
            raise SystemExit(65)
    if not isinstance(value, str) or not value or any(ch.isspace() for ch in value):
        raise SystemExit(65)
    sys.stdout.write(value)
    raise SystemExit(0)
raise SystemExit(66)
PY
    )"
    if valid_token "$raw_pat"; then
      GH_TOKEN="$raw_pat"
      credential_source="protected-gh-profile-file:${profile_path}"
      break
    fi
  done
fi
unset raw_pat ec2_uid profile_record profile_path profile_owner profile_mode profile_expected_uid

if test -z "$GH_TOKEN"; then
  printf 'publisher-stage=%s status=failed reason=no-readable-protected-credential\n' "$stage" >&2
  exit 65
fi
export GH_TOKEN
export GITHUB_REPOSITORY_ADMIN_TOKEN="$GH_TOKEN"
printf 'publisher-stage=%s status=passed source=%s\n' "$stage" "$credential_source" >&2

stage=trusted-source
required_paths=(
  scripts/ops/bootstrap_org_dotgithub_repositories.py
  scripts/ops/bootstrap_org_dotgithub_repositories_hardened.py
  tests/ops/test_bootstrap_org_dotgithub_repositories.py
  tests/ops/test_bootstrap_org_dotgithub_repositories_hardened.py
)
for relative_path in "${required_paths[@]}"; do
  test -f "$source_root/$relative_path"
done
printf 'publisher-stage=%s status=passed sha=%s\n' "$stage" "$trusted_sha" >&2

stage=publisher-validation
python3 -m py_compile \
  "$source_root/scripts/ops/bootstrap_org_dotgithub_repositories.py" \
  "$source_root/scripts/ops/bootstrap_org_dotgithub_repositories_hardened.py"
python3 -m unittest discover \
  -s "$source_root/tests/ops" \
  -p 'test_bootstrap_org_dotgithub_repositories*.py' \
  -v >&2
printf 'publisher-stage=%s status=passed\n' "$stage" >&2

stage=bounded-publication
json_report="$runtime_dir/org-dotgithub-governance.json"
markdown_report="$runtime_dir/org-dotgithub-governance.md"
publisher_log="$runtime_dir/org-dotgithub-governance.log"

set +e
python3 "$source_root/scripts/ops/bootstrap_org_dotgithub_repositories_hardened.py" \
  --execute \
  --json-report "$json_report" \
  --markdown-report "$markdown_report" \
  >"$publisher_log" 2>&1
publisher_status=$?
set -e

if test "$publisher_status" -ne 0; then
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

REPORT="$json_report" python3 - <<'PY'
import json
import os
from pathlib import Path

payload = json.loads(Path(os.environ["REPORT"]).read_text(encoding="utf-8"))
organizations = payload.get("organizations")
if payload.get("mode") != "execute" or not isinstance(organizations, list) or len(organizations) != 36:
    raise SystemExit("publisher report does not certify the fixed 36-organization execute fleet")
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
PY

test -s "$markdown_report"
cleanup_environment
printf 'publisher-stage=%s status=passed\n' "$stage" >&2
cat "$markdown_report"
printf '\n<!-- org-dotgithub-governance-report-complete -->\n'

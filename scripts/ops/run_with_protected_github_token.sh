#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly TARGET_ORG='networking-components'
readonly EXPECTED_LOGIN='ORESoftware'
readonly API_URL='https://api.github.com'
readonly API_VERSION='2022-11-28'

stage='initialization'
resolved_token=''
credential_source=''
profile_diagnostic='not-checked'
publisher_region="${AWS_REGION:-${AWS_DEFAULT_REGION:-us-east-1}}"

fail() {
  printf 'protected-github-token-runner stage=%s status=failed message=%s\n' "$stage" "$*" >&2
  exit 1
}

cleanup() {
  unset resolved_token raw_token encoded_token secret_json credential_source
  unset PROTECTED_GITHUB_TOKEN GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN
  unset profile_path profile_owner profile_mode profile_expected_uid profile_record
  unset ec2_home ec2_uid publisher_region
}
trap cleanup EXIT INT TERM
trap 'fail "unexpected command failure at line ${LINENO}"' ERR

[[ "$publisher_region" =~ ^[a-z]{2}(-gov)?-[a-z0-9-]+-[0-9]$ ]] || fail 'invalid AWS region'
[[ $# -ge 1 ]] || fail 'an executable command is required'
command_path="$1"
shift
[[ "$command_path" == /* ]] || fail 'command path must be absolute'
[[ -f "$command_path" && -x "$command_path" ]] || fail 'command path is not an executable regular file'

valid_token() {
  local candidate="${1:-}"
  [[ ${#candidate} -ge 20 ]] || return 1
  [[ "$candidate" != *$'\n'* ]] || return 1
  [[ "$candidate" != *$'\r'* ]] || return 1
  [[ "$candidate" != *$'\t'* ]] || return 1
  [[ "$candidate" != *' '* ]] || return 1
}

stage='resolve-protected-credential'

# Keep the credential on the protected SSM host. This first path mirrors the
# External Secrets source but deliberately treats a missing or rotated GH_PAT
# field as a normal fallback condition.
if command -v aws >/dev/null 2>&1; then
  secret_json="$(
    aws secretsmanager get-secret-value \
      --region "$publisher_region" \
      --secret-id dd/remote-dev/agent-secrets \
      --query SecretString \
      --output text 2>/dev/null || true
  )"
  if [[ -n "$secret_json" ]]; then
    raw_token="$(
      printf '%s' "$secret_json" | python3 -c '
import json
import sys
try:
    payload = json.load(sys.stdin)
except (json.JSONDecodeError, OSError):
    raise SystemExit(0)
value = payload.get("GH_PAT")
if isinstance(value, str) and value:
    sys.stdout.write(value)
' 2>/dev/null || true
    )"
    if valid_token "$raw_token"; then
      resolved_token="$raw_token"
      credential_source='aws-secrets-manager'
    fi
  fi
fi
unset raw_token secret_json

# Fall back to the reconciled Kubernetes Secret. The token is decoded only in
# process memory and is never printed or passed as a command-line argument.
if [[ -z "$resolved_token" ]] && command -v kubectl >/dev/null 2>&1; then
  for kubeconfig in \
    /etc/kubernetes/admin.conf \
    /root/.kube/config \
    /home/ec2-user/.kube/config
  do
    [[ -r "$kubeconfig" ]] || continue
    encoded_token="$(
      KUBECONFIG="$kubeconfig" \
        kubectl -n default get secret dd-agent-secrets \
        -o jsonpath='{.data.GH_PAT}' 2>/dev/null || true
    )"
    [[ -n "$encoded_token" ]] || continue
    raw_token="$(printf '%s' "$encoded_token" | base64 --decode 2>/dev/null || true)"
    unset encoded_token
    if valid_token "$raw_token"; then
      resolved_token="$raw_token"
      credential_source="kubernetes-secret:${kubeconfig}"
      break
    fi
  done
fi
unset raw_token encoded_token

# Prefer gh's credential abstraction when the protected account profile is
# available. Explicit HOME/XDG values prevent shell startup files from moving
# the credential boundary.
if [[ -z "$resolved_token" ]] && \
   command -v sudo >/dev/null 2>&1 && \
   command -v getent >/dev/null 2>&1; then
  ec2_home="$(getent passwd ec2-user | awk -F: '$1 == "ec2-user" { print $6 }')"
  case "$ec2_home" in
    /*)
      raw_token="$(
        sudo -u ec2-user -H \
          env \
            -u GH_TOKEN \
            -u GITHUB_TOKEN \
            -u GH_ENTERPRISE_TOKEN \
            -u GITHUB_REPOSITORY_ADMIN_TOKEN \
            -u PROTECTED_GITHUB_TOKEN \
            -u GH_CONFIG_DIR \
            HOME="$ec2_home" \
            XDG_CONFIG_HOME="$ec2_home/.config" \
            bash -c 'command -v gh >/dev/null 2>&1 && gh auth token --hostname github.com' \
          2>/dev/null || true
      )"
      ;;
    *) raw_token='' ;;
  esac
  if valid_token "$raw_token"; then
    resolved_token="$raw_token"
    credential_source='protected-gh-profile'
  fi
fi
unset raw_token ec2_home

# A canonical file-backed gh profile may remain after the binary is removed.
# Reject symlinks, unexpected owners, and group/world-writable files before
# parsing only github.com's oauth_token field.
if [[ -z "$resolved_token" ]] && \
   command -v python3 >/dev/null 2>&1 && \
   command -v stat >/dev/null 2>&1; then
  profile_diagnostic='no-canonical-profile'
  ec2_uid=''
  if command -v id >/dev/null 2>&1; then
    ec2_uid="$(id -u ec2-user 2>/dev/null || true)"
  fi

  for profile_record in \
    '/root/.config/gh/hosts.yml:0' \
    "/home/ec2-user/.config/gh/hosts.yml:${ec2_uid}"
  do
    profile_path="${profile_record%:*}"
    profile_expected_uid="${profile_record##*:}"
    [[ -n "$profile_expected_uid" ]] || continue
    [[ -f "$profile_path" ]] || continue
    if [[ -L "$profile_path" || ! -r "$profile_path" ]]; then
      profile_diagnostic='canonical-profile-unreadable-or-symlink'
      continue
    fi

    profile_owner="$(stat -c '%u' "$profile_path" 2>/dev/null || true)"
    profile_mode="$(stat -c '%a' "$profile_path" 2>/dev/null || true)"
    if [[ "$profile_owner" != "$profile_expected_uid" ]] || \
       [[ ! "$profile_mode" =~ ^[0-7]{3,4}$ ]] || \
       (( (8#$profile_mode & 0022) != 0 )); then
      profile_diagnostic='canonical-profile-ownership-or-mode-rejected'
      continue
    fi

    raw_token="$(
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
    if valid_token "$raw_token"; then
      resolved_token="$raw_token"
      credential_source="protected-gh-profile-file:${profile_path}"
      profile_diagnostic='canonical-profile-token-loaded'
      break
    fi
    profile_diagnostic='canonical-profile-has-no-file-backed-token'
  done
fi
unset raw_token ec2_uid profile_record profile_path profile_owner profile_mode profile_expected_uid

if [[ -z "$resolved_token" ]]; then
  aws_diagnostic='aws-cli-absent'
  if command -v aws >/dev/null 2>&1; then
    if aws secretsmanager get-secret-value \
      --region "$publisher_region" \
      --secret-id dd/remote-dev/agent-secrets \
      --query SecretString \
      --output text >/dev/null 2>&1; then
      aws_diagnostic='secret-readable-but-gh-pat-unusable'
    else
      aws_diagnostic='secret-unavailable-or-denied'
    fi
  fi

  kube_diagnostic='kubectl-absent'
  if command -v kubectl >/dev/null 2>&1; then
    kube_diagnostic='no-readable-kubeconfig'
    for diagnostic_kubeconfig in \
      /etc/kubernetes/admin.conf \
      /root/.kube/config \
      /home/ec2-user/.kube/config
    do
      [[ -r "$diagnostic_kubeconfig" ]] || continue
      kube_diagnostic='secret-unavailable-or-empty'
      if KUBECONFIG="$diagnostic_kubeconfig" \
        kubectl -n default get secret dd-agent-secrets \
        -o jsonpath='{.data.GH_PAT}' 2>/dev/null | grep -q .; then
        kube_diagnostic='encoded-gh-pat-present-but-unusable'
      fi
      break
    done
  fi

  gh_diagnostic='prerequisites-absent'
  if command -v sudo >/dev/null 2>&1 && command -v getent >/dev/null 2>&1; then
    diagnostic_home="$(getent passwd ec2-user | awk -F: '$1 == "ec2-user" { print $6 }')"
    case "$diagnostic_home" in
      /*)
        if sudo -u ec2-user -H env \
          -u GH_TOKEN \
          -u GITHUB_TOKEN \
          -u GH_ENTERPRISE_TOKEN \
          -u GITHUB_REPOSITORY_ADMIN_TOKEN \
          -u PROTECTED_GITHUB_TOKEN \
          -u GH_CONFIG_DIR \
          HOME="$diagnostic_home" \
          XDG_CONFIG_HOME="$diagnostic_home/.config" \
          bash -c 'command -v gh >/dev/null 2>&1'; then
          if sudo -u ec2-user -H env \
            -u GH_TOKEN \
            -u GITHUB_TOKEN \
            -u GH_ENTERPRISE_TOKEN \
            -u GITHUB_REPOSITORY_ADMIN_TOKEN \
            -u PROTECTED_GITHUB_TOKEN \
            -u GH_CONFIG_DIR \
            HOME="$diagnostic_home" \
            XDG_CONFIG_HOME="$diagnostic_home/.config" \
            bash -c 'gh auth status --hostname github.com >/dev/null 2>&1'; then
            gh_diagnostic='auth-valid-but-token-unavailable'
          else
            gh_diagnostic='auth-unavailable'
          fi
        else
          gh_diagnostic='gh-cli-absent'
        fi
        ;;
      *) gh_diagnostic='ec2-home-unresolved' ;;
    esac
  fi
  unset diagnostic_home diagnostic_kubeconfig

  printf 'protected-github-token-runner stage=%s status=failed reason=no-readable-protected-credential aws=%s kubernetes=%s gh=%s profile=%s\n' \
    "$stage" "$aws_diagnostic" "$kube_diagnostic" "$gh_diagnostic" "$profile_diagnostic" >&2
  exit 65
fi

stage='validate-identity-and-ownership'
RESOLVED_GITHUB_TOKEN="$resolved_token" python3 - <<'PY'
import json
import os
import urllib.error
import urllib.request

API = "https://api.github.com"
TOKEN = os.environ["RESOLVED_GITHUB_TOKEN"]
HEADERS = {
    "Accept": "application/vnd.github+json",
    "Authorization": f"Bearer {TOKEN}",
    "X-GitHub-Api-Version": "2022-11-28",
    "User-Agent": "networking-components-protected-token-runner",
}

def get(path: str) -> object:
    request = urllib.request.Request(API + path, headers=HEADERS)
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        detail = error.read(2048).decode(errors="replace")
        if TOKEN:
            detail = detail.replace(TOKEN, "[REDACTED]")
        raise SystemExit(f"GitHub preflight failed for {path}: HTTP {error.code}: {detail}") from error

identity = get("/user")
if not isinstance(identity, dict) or identity.get("login") != "ORESoftware":
    observed = identity.get("login") if isinstance(identity, dict) else None
    raise SystemExit(f"unexpected protected publisher identity: {observed!r}")

membership = get("/user/memberships/orgs/networking-components")
if not isinstance(membership, dict):
    raise SystemExit("organization membership response is malformed")
observed = (membership.get("role"), membership.get("state"))
if observed != ("admin", "active"):
    raise SystemExit(f"networking-components owner membership is {observed!r}")
PY
unset RESOLVED_GITHUB_TOKEN

printf 'protected-github-token-runner stage=%s status=passed source=%s\n' \
  "$stage" "$credential_source"

stage='execute-reviewed-command'
export PROTECTED_GITHUB_TOKEN="$resolved_token"
unset resolved_token
exec "$command_path" "$@"

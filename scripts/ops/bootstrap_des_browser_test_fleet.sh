#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly TARGET_ORG='discrete-event-systems-test'
readonly EXPECTED_LOGIN='ORESoftware'
readonly PROJECT_TITLE='DES Browser Automation'

stage='initialization'
github_token=''
gateway_auth=''
credential_source=''
gateway_source=''

fail() {
  printf 'des-browser-bootstrap stage=%s status=failed message=%s\n' "$stage" "$*" >&2
  exit 1
}

cleanup() {
  unset github_token gateway_auth raw_value encoded_value secret_json
  unset GH_TOKEN GITHUB_TOKEN DES_GATEWAY_AUTH
}
trap cleanup EXIT INT TERM
trap 'fail "unexpected command failure at line ${LINENO}"' ERR

valid_credential() {
  local candidate="${1:-}"
  [[ ${#candidate} -ge 20 ]] || return 1
  [[ "$candidate" != *$'\n'* ]] || return 1
  [[ "$candidate" != *$'\r'* ]] || return 1
  [[ "$candidate" != *$'\t'* ]] || return 1
  [[ "$candidate" != *' '* ]] || return 1
}

json_secret_key() {
  local secret_id="$1"
  local key="$2"
  command -v aws >/dev/null 2>&1 || return 0
  secret_json="$(
    aws secretsmanager get-secret-value \
      --region "${AWS_REGION:-${AWS_DEFAULT_REGION:-us-east-1}}" \
      --secret-id "$secret_id" \
      --query SecretString \
      --output text 2>/dev/null || true
  )"
  [[ -n "$secret_json" ]] || return 0
  SECRET_KEY="$key" python3 -c '
import json
import os
import sys
try:
    payload = json.load(sys.stdin)
except (json.JSONDecodeError, OSError):
    raise SystemExit(0)
value = payload.get(os.environ["SECRET_KEY"])
if isinstance(value, str) and value:
    sys.stdout.write(value)
' <<<"$secret_json" 2>/dev/null || true
}

kubernetes_secret_key() {
  local secret_name="$1"
  local key="$2"
  local kubeconfig
  for kubeconfig in \
    /etc/kubernetes/admin.conf \
    /root/.kube/config \
    /home/ec2-user/.kube/config
  do
    [[ -r "$kubeconfig" ]] || continue
    encoded_value="$(
      KUBECONFIG="$kubeconfig" \
        kubectl -n default get secret "$secret_name" \
        -o "jsonpath={.data.${key}}" 2>/dev/null || true
    )"
    [[ -n "$encoded_value" ]] || continue
    raw_value="$(printf '%s' "$encoded_value" | base64 --decode 2>/dev/null || true)"
    unset encoded_value
    if valid_credential "$raw_value"; then
      printf '%s' "$raw_value"
      return 0
    fi
  done
  return 0
}

profile_token() {
  local profile
  for profile in /root/.config/gh/hosts.yml /home/ec2-user/.config/gh/hosts.yml; do
    [[ -f "$profile" && -r "$profile" && ! -L "$profile" ]] || continue
    GH_HOSTS_PROFILE="$profile" python3 - <<'PY' 2>/dev/null || true
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
        value = ast.literal_eval(value)
    if isinstance(value, str) and value and not any(ch.isspace() for ch in value):
        sys.stdout.write(value)
        raise SystemExit(0)
raise SystemExit(0)
PY
  done
}

stage='tool-preflight'
command -v gh >/dev/null 2>&1 || fail 'gh is unavailable'
command -v kubectl >/dev/null 2>&1 || fail 'kubectl is unavailable'
command -v python3 >/dev/null 2>&1 || fail 'python3 is unavailable'
command -v base64 >/dev/null 2>&1 || fail 'base64 is unavailable'
printf 'des-browser-bootstrap stage=%s status=passed\n' "$stage"

stage='github-credential'
raw_value="$(json_secret_key 'dd/remote-dev/agent-secrets' 'GH_PAT')"
if valid_credential "$raw_value"; then
  github_token="$raw_value"
  credential_source='aws-secrets-manager'
fi
unset raw_value secret_json

if [[ -z "$github_token" ]]; then
  raw_value="$(kubernetes_secret_key 'dd-agent-secrets' 'GH_PAT')"
  if valid_credential "$raw_value"; then
    github_token="$raw_value"
    credential_source='kubernetes-secret'
  fi
fi
unset raw_value

if [[ -z "$github_token" ]]; then
  raw_value="$(profile_token)"
  if valid_credential "$raw_value"; then
    github_token="$raw_value"
    credential_source='protected-gh-profile'
  fi
fi
unset raw_value

valid_credential "$github_token" || fail 'no usable protected GitHub credential was resolved'
export GH_TOKEN="$github_token" GITHUB_TOKEN="$github_token"
printf 'des-browser-bootstrap stage=%s status=passed source=%s\n' "$stage" "$credential_source"

stage='gateway-credential'
raw_value="$(kubernetes_secret_key 'dd-remote-auth-secrets' 'DD_AUTH_COOKIE_VALUE')"
if valid_credential "$raw_value"; then
  gateway_auth="$raw_value"
  gateway_source='kubernetes-secret'
fi
unset raw_value

if [[ -z "$gateway_auth" ]]; then
  for secret_id in \
    dd/remote-dev/auth-secrets \
    dd/remote-dev/remote-auth-secrets \
    dd/remote-dev/gateway-auth-secrets
  do
    raw_value="$(json_secret_key "$secret_id" 'DD_AUTH_COOKIE_VALUE')"
    if valid_credential "$raw_value"; then
      gateway_auth="$raw_value"
      gateway_source="aws-secrets-manager:${secret_id}"
      break
    fi
  done
fi
unset raw_value secret_json
valid_credential "$gateway_auth" || fail 'no usable gateway credential was resolved'
export DES_GATEWAY_AUTH="$gateway_auth"
printf 'des-browser-bootstrap stage=%s status=passed source=%s\n' "$stage" "$gateway_source"

stage='github-identity'
[[ "$(gh api user --jq .login)" == "$EXPECTED_LOGIN" ]] || fail 'unexpected GitHub publisher identity'
[[ "$(gh api "user/memberships/orgs/${TARGET_ORG}" --jq '.role + ":" + .state')" == 'admin:active' ]] \
  || fail "publisher is not an active owner of ${TARGET_ORG}"
printf 'des-browser-bootstrap stage=%s status=passed\n' "$stage"

ensure_repo() {
  local name="$1"
  local description="$2"
  local full="${TARGET_ORG}/${name}"
  stage="repository:${full}"
  if ! gh repo view "$full" >/dev/null 2>&1; then
    gh repo create "$full" --public --description "$description" --add-readme
  fi
  gh api -X PATCH "repos/${full}" \
    -f has_issues=true \
    -f has_projects=true \
    -f has_wiki=false \
    -f delete_branch_on_merge=true >/dev/null
  if [[ "$name" != '.github' ]]; then
    gh secret set DES_GATEWAY_AUTH --repo "$full" --body "$gateway_auth"
  fi
  printf 'des-browser-bootstrap stage=%s status=passed\n' "$stage"
}

ensure_project() {
  local owner="$1"
  local number
  stage="project:${owner}"
  number="$(
    gh project list --owner "$owner" --format json \
      --jq ".projects[] | select(.title == \"${PROJECT_TITLE}\") | .number" \
      | head -1
  )"
  if [[ -z "$number" ]]; then
    number="$(
      gh project create --owner "$owner" --title "$PROJECT_TITLE" \
        --format json --jq .number
    )"
  fi
  printf 'des-browser-bootstrap stage=%s status=passed url=https://github.com/orgs/%s/projects/%s\n' \
    "$stage" "$owner" "$number"
}

ensure_repo des-web-playwright-e2e \
  'Playwright browser contracts for des-web across GitHub Actions and gha-indie-worker.'
ensure_repo des-web-puppeteer-e2e \
  'Puppeteer browser contracts for des-web across GitHub Actions and gha-indie-worker.'
ensure_repo .github \
  'Organization profile and DES browser-test fleet documentation.'
ensure_project discrete-event-systems
ensure_project discrete-event-systems-test

stage='complete'
printf 'des-browser-bootstrap stage=%s status=success\n' "$stage"

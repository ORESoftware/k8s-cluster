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
app_work=''
app_evidence=''

fail() {
  printf 'des-browser-bootstrap stage=%s status=failed message=%s\n' "$stage" "$*" >&2
  exit 1
}

cleanup() {
  unset github_token gateway_auth raw_value encoded_value secret_json candidate profile_home
  unset GH_TOKEN GITHUB_TOKEN DES_GATEWAY_AUTH DES_APP_SELECTOR DES_APP_TOKEN_HELPER
  if [[ -n "$app_work" ]]; then
    rm -rf "$app_work"
  fi
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

gh_cli_token() {
  local candidate=''
  local profile_home=''

  candidate="$(
    env \
      -u GH_TOKEN \
      -u GITHUB_TOKEN \
      -u GH_ENTERPRISE_TOKEN \
      -u GITHUB_REPOSITORY_ADMIN_TOKEN \
      gh auth token --hostname github.com 2>/dev/null || true
  )"
  if valid_credential "$candidate"; then
    printf '%s' "$candidate"
    return 0
  fi

  if command -v sudo >/dev/null 2>&1 && command -v getent >/dev/null 2>&1; then
    profile_home="$(getent passwd ec2-user | awk -F: '$1 == "ec2-user" {print $6}')"
    if [[ "$profile_home" == /* ]]; then
      candidate="$(
        sudo -u ec2-user -H env \
          -u GH_TOKEN \
          -u GITHUB_TOKEN \
          -u GH_ENTERPRISE_TOKEN \
          -u GITHUB_REPOSITORY_ADMIN_TOKEN \
          PATH="$PATH" \
          HOME="$profile_home" \
          XDG_CONFIG_HOME="$profile_home/.config" \
          gh auth token --hostname github.com 2>/dev/null || true
      )"
      if valid_credential "$candidate"; then
        printf '%s' "$candidate"
        return 0
      fi
    fi
  fi
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
command -v openssl >/dev/null 2>&1 || fail 'openssl is unavailable'
printf 'des-browser-bootstrap stage=%s status=passed\n' "$stage"

stage='github-credential'
raw_value="$(gh_cli_token)"
if valid_credential "$raw_value"; then
  github_token="$raw_value"
  credential_source='authenticated-gh-cli'
fi
unset raw_value

if [[ -z "$github_token" ]]; then
  raw_value="$(json_secret_key 'dd/remote-dev/agent-secrets' 'GH_PAT')"
  if valid_credential "$raw_value"; then
    github_token="$raw_value"
    credential_source='aws-secrets-manager'
  fi
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
    credential_source='protected-gh-profile-file'
  fi
fi
unset raw_value

if [[ -z "$github_token" ]] && \
   [[ -f "${DES_APP_SELECTOR:-}" ]] && \
   [[ -f "${DES_APP_TOKEN_HELPER:-}" ]]; then
  app_work="$(mktemp -d /tmp/des-browser-github-app.XXXXXX)"
  app_evidence="$app_work/evidence.json"
  python3 "$DES_APP_TOKEN_HELPER" \
    --selector "$DES_APP_SELECTOR" \
    --organization "$TARGET_ORG" \
    --token-out "$app_work/installation-token" \
    --evidence-out "$app_evidence" \
    --region "${AWS_REGION:-${AWS_DEFAULT_REGION:-us-east-1}}"
  raw_value="$(cat "$app_work/installation-token")"
  if valid_credential "$raw_value"; then
    github_token="$raw_value"
    credential_source='github-app-installation'
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
if valid_credential "$gateway_auth"; then
  export DES_GATEWAY_AUTH="$gateway_auth"
  printf 'des-browser-bootstrap stage=%s status=passed source=%s\n' "$stage" "$gateway_source"
else
  gateway_auth=''
  printf 'des-browser-bootstrap stage=%s status=skipped reason=credential-unavailable\n' "$stage"
fi

stage='github-identity'
if [[ "$credential_source" == 'github-app-installation' ]]; then
  python3 - "$app_evidence" "$TARGET_ORG" <<'PY'
import json
import sys
from pathlib import Path

evidence = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
if evidence.get("organization") != sys.argv[2]:
    raise SystemExit("GitHub App evidence targets the wrong organization")
required = {
    "administration": "write",
    "contents": "write",
    "pull_requests": "write",
    "metadata": "read",
}
if evidence.get("token_permissions") != required:
    raise SystemExit("GitHub App evidence has insufficient permissions")
PY
  gh api installation/repositories --jq '.total_count' >/dev/null
else
  [[ "$(gh api user --jq .login)" == "$EXPECTED_LOGIN" ]] || fail 'unexpected GitHub publisher identity'
  [[ "$(gh api "user/memberships/orgs/${TARGET_ORG}" --jq '.role + ":" + .state')" == 'admin:active' ]] \
    || fail "publisher is not an active owner of ${TARGET_ORG}"
fi
printf 'des-browser-bootstrap stage=%s status=passed source=%s\n' "$stage" "$credential_source"

ensure_repo() {
  local name="$1"
  local description="$2"
  local full="${TARGET_ORG}/${name}"
  stage="repository:${full}"
  if ! gh repo view "$full" >/dev/null 2>&1; then
    gh repo create "$full" --public --description "$description" --add-readme
  fi
  gh api -X PATCH "repos/${full}" \
    -F has_issues=true \
    -F has_projects=true \
    -F has_wiki=false \
    -F delete_branch_on_merge=true >/dev/null
  if [[ "$name" != '.github' && -n "$gateway_auth" && "$credential_source" != 'github-app-installation' ]]; then
    gh secret set DES_GATEWAY_AUTH --repo "$full" --body "$gateway_auth"
  fi
  printf 'des-browser-bootstrap stage=%s status=passed\n' "$stage"
}

ensure_project() {
  local owner="$1"
  local number=''
  stage="project:${owner}"
  if [[ "$credential_source" == 'github-app-installation' ]]; then
    printf 'des-browser-bootstrap stage=%s status=deferred reason=user-project-token-required\n' "$stage"
    return 0
  fi
  number="$(
    gh project list --owner "$owner" --format json \
      --jq ".projects[] | select(.title == \"${PROJECT_TITLE}\") | .number" \
      2>/dev/null | head -1 || true
  )"
  if [[ -z "$number" ]]; then
    number="$(
      gh project create --owner "$owner" --title "$PROJECT_TITLE" \
        --format json --jq .number 2>/dev/null || true
    )"
  fi
  if [[ -n "$number" ]]; then
    printf 'des-browser-bootstrap stage=%s status=passed url=https://github.com/orgs/%s/projects/%s\n' \
      "$stage" "$owner" "$number"
  else
    printf 'des-browser-bootstrap stage=%s status=deferred reason=project-scope-unavailable\n' "$stage"
  fi
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
printf 'des-browser-bootstrap stage=%s status=success credential_source=%s\n' "$stage" "$credential_source"

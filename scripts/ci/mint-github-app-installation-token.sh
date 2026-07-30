#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 <owner> <token-output-file> <repository-name> [repository-name ...]" >&2
  exit 64
}

(( $# >= 3 )) || usage
owner="$1"
token_output="$2"
shift 2
repositories=("$@")

if [[ ! "$owner" =~ ^[A-Za-z0-9]([A-Za-z0-9-]{0,37}[A-Za-z0-9])?$ ]]; then
  echo "::error title=Invalid GitHub owner::${owner} is not a valid GitHub account name"
  exit 64
fi

for repository in "${repositories[@]}"; do
  if [[ ! "$repository" =~ ^[A-Za-z0-9_.-]{1,100}$ ]] || [[ "$repository" == "." || "$repository" == ".." ]]; then
    echo "::error title=Invalid GitHub repository::${repository} is not a safe repository name"
    exit 64
  fi
done

app_id="${K8S_SUBMODULE_APP_ID:-}"
private_key="${K8S_SUBMODULE_APP_PRIVATE_KEY:-}"
api_url="${GITHUB_API_URL:-https://api.github.com}"
api_version="${GITHUB_API_VERSION:-2026-03-10}"

if [[ ! "$app_id" =~ ^[0-9]+$ ]]; then
  echo "::error title=GitHub App ID missing::K8S_SUBMODULE_APP_ID must be the numeric App ID"
  exit 2
fi
if [[ -z "$private_key" ]]; then
  echo "::error title=GitHub App private key missing::K8S_SUBMODULE_APP_PRIVATE_KEY is required"
  exit 2
fi
if [[ "$api_url" != https://* && "$api_url" != http://127.0.0.1:* && "$api_url" != http://localhost:* && "$api_url" != http://\[::1\]:* ]]; then
  echo "::error title=Unsafe GitHub API URL::GITHUB_API_URL must use HTTPS, except loopback HTTP is allowed for tests"
  exit 64
fi
api_url="${api_url%/}"

work_dir="$(mktemp -d "${RUNNER_TEMP:-/tmp}/github-app-token.XXXXXX")"
private_key_file="${work_dir}/app-private-key.pem"
installation_response="${work_dir}/installation.json"
token_request="${work_dir}/token-request.json"
token_response="${work_dir}/token-response.json"

cleanup() {
  if [[ -d "$work_dir" ]]; then
    find "$work_dir" -depth -delete
  fi
}
trap cleanup EXIT

umask 077
printf '%s' "$private_key" >"$private_key_file"
if ! grep -Eq '^-----BEGIN (RSA )?PRIVATE KEY-----$' "$private_key_file"; then
  echo "::error title=Invalid GitHub App private key::K8S_SUBMODULE_APP_PRIVATE_KEY must contain a PEM private key"
  exit 2
fi

base64url() {
  openssl base64 -A | tr '+/' '-_' | tr -d '='
}

now="$(date +%s)"
iat="$((now - 60))"
exp="$((now + 540))"
header='{"alg":"RS256","typ":"JWT"}'
payload="$(printf '{"iat":%s,"exp":%s,"iss":%s}' "$iat" "$exp" "$app_id")"
unsigned_token="$(printf '%s' "$header" | base64url).$(printf '%s' "$payload" | base64url)"
signature="$(printf '%s' "$unsigned_token" | openssl dgst -sha256 -sign "$private_key_file" -binary | base64url)"
app_jwt="${unsigned_token}.${signature}"

api_error_message() {
  local response_file="$1"
  python3 - "$response_file" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
try:
    payload = json.loads(path.read_text(encoding="utf-8"))
    message = str(payload.get("message", "GitHub API request failed"))
except Exception:
    message = "GitHub API request failed"
print("".join(ch for ch in message if ch.isprintable())[:300])
PY
}

first_repository="${repositories[0]}"
installation_status="$(
  curl --silent --show-error \
    --output "$installation_response" \
    --write-out '%{http_code}' \
    --header 'Accept: application/vnd.github+json' \
    --header "Authorization: Bearer ${app_jwt}" \
    --header "X-GitHub-Api-Version: ${api_version}" \
    "${api_url}/repos/${owner}/${first_repository}/installation"
)" || {
  echo "::error title=GitHub App installation lookup failed::network failure for ${owner}/${first_repository}"
  exit 1
}

if [[ "$installation_status" != "200" ]]; then
  message="$(api_error_message "$installation_response")"
  echo "::error title=GitHub App installation unavailable::${owner}/${first_repository} returned HTTP ${installation_status}: ${message}"
  exit 1
fi

installation_id="$(python3 - "$installation_response" <<'PY'
import json
import pathlib
import sys

payload = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
installation_id = payload.get("id")
if not isinstance(installation_id, int) or installation_id <= 0:
    raise SystemExit("installation response did not contain a valid id")
print(installation_id)
PY
)" || {
  echo "::error title=Invalid GitHub App installation response::GitHub did not return a valid installation ID for ${owner}"
  exit 1
}

python3 - "$token_request" "${repositories[@]}" <<'PY'
import json
import pathlib
import sys

output = pathlib.Path(sys.argv[1])
repositories = sys.argv[2:]
output.write_text(json.dumps({
    "repositories": repositories,
    "permissions": {"contents": "read"},
}, separators=(",", ":")), encoding="utf-8")
PY

token_status="$(
  curl --silent --show-error \
    --request POST \
    --output "$token_response" \
    --write-out '%{http_code}' \
    --header 'Accept: application/vnd.github+json' \
    --header "Authorization: Bearer ${app_jwt}" \
    --header "X-GitHub-Api-Version: ${api_version}" \
    --header 'Content-Type: application/json' \
    --data-binary "@${token_request}" \
    "${api_url}/app/installations/${installation_id}/access_tokens"
)" || {
  echo "::error title=GitHub App token request failed::network failure while minting a token for ${owner}"
  exit 1
}

if [[ "$token_status" != "201" ]]; then
  message="$(api_error_message "$token_response")"
  echo "::error title=GitHub App token unavailable::${owner} returned HTTP ${token_status}: ${message}"
  exit 1
fi

readarray -t token_fields < <(python3 - "$token_response" <<'PY'
import json
import pathlib
import sys

payload = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
token = payload.get("token")
expires_at = payload.get("expires_at")
permissions = payload.get("permissions", {})
if not isinstance(token, str) or not token:
    raise SystemExit("token response did not contain a token")
if permissions.get("contents") != "read":
    raise SystemExit("token did not receive contents:read")
print(token)
print(expires_at or "unknown")
PY
) || {
  echo "::error title=Invalid GitHub App token response::GitHub did not return a contents-read installation token for ${owner}"
  exit 1
}

installation_token="${token_fields[0]}"
expires_at="${token_fields[1]}"
if [[ -n "${GITHUB_ACTIONS:-}" ]]; then
  printf '::add-mask::%s\n' "$installation_token"
fi
mkdir -p "$(dirname "$token_output")"
printf '%s' "$installation_token" >"$token_output"
chmod 600 "$token_output"

unset app_jwt installation_token private_key
printf 'Minted a contents-read installation token for %s covering %d repository/repositories; expires %s.\n' \
  "$owner" "${#repositories[@]}" "$expires_at"

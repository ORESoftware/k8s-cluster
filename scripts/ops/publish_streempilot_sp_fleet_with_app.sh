#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly ORG='StreemPilot'
readonly API_URL='https://api.github.com'
readonly API_VERSION='2022-11-28'
readonly EXPECTED_REPOSITORIES=8
readonly EXPECTED_PULL_REQUESTS=8
readonly ENCODED_SHA256='59c1bc4595d340ab9af0e51b9ba8ea4baea0b250346e3d25d8f5d5e607fe624e'
readonly GENERATOR_SHA256='10188d2d4018dec7a28ff3e85a6779f67517864d7d85b5d03d769402a3a55fd7'
readonly REPORT_JSON="${RUNNER_TEMP:-/tmp}/streempilot-sp-app-publication.json"
readonly REPORT_MARKDOWN="${RUNNER_TEMP:-/tmp}/streempilot-sp-app-publication.md"

stage='bootstrap'
work=''
app_jwt=''
installation_token=''

fail() {
  printf 'streempilot-app-publisher stage=%s status=failed message=%s\n' "$stage" "$*" >&2
  exit 1
}

cleanup() {
  local status=$?
  if [[ -n "$installation_token" ]]; then
    curl --silent --show-error \
      --request DELETE \
      --output /dev/null \
      --header 'Accept: application/vnd.github+json' \
      --header "Authorization: Bearer ${installation_token}" \
      --header "X-GitHub-Api-Version: ${API_VERSION}" \
      "${API_URL}/installation/token" || true
  fi
  unset GH_TOKEN installation_token app_jwt K8S_SUBMODULE_APP_PRIVATE_KEY K8S_SUBMODULE_APP_ID
  if [[ -n "$work" && -e "$work" ]]; then
    python3 - "$work" <<'PY'
import shutil
import sys
from pathlib import Path
path = Path(sys.argv[1])
if path.exists():
    shutil.rmtree(path)
PY
  fi
  exit "$status"
}
trap cleanup EXIT INT TERM
trap 'fail "unexpected command failure at line ${LINENO}"' ERR

: "${K8S_SUBMODULE_APP_ID:?K8S_SUBMODULE_APP_ID is required}"
: "${K8S_SUBMODULE_APP_PRIVATE_KEY:?K8S_SUBMODULE_APP_PRIVATE_KEY is required}"
[[ "$K8S_SUBMODULE_APP_ID" =~ ^[0-9]+$ ]] || fail 'GitHub App ID must be numeric'
[[ -z "${GH_PAT:-}" ]] || fail 'GH_PAT must not be present in this App-only publisher'
[[ -z "${GITHUB_REPOSITORY_ADMIN_TOKEN:-}" ]] || fail 'repository-admin PAT must not be present'

for command in base64 curl gh git jq node npm openssl python3 sha256sum xz; do
  command -v "$command" >/dev/null || fail "required command unavailable: $command"
done

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
parts=("$repo_root"/scripts/ops/streempilot-generator/generate_streempilot_sp_fleet.py.xz.b64.part*)
[[ "${#parts[@]}" == 7 ]] || fail "unexpected generator chunk count: ${#parts[@]}"

work="$(mktemp -d "${RUNNER_TEMP:-/tmp}/streempilot-app-publisher.XXXXXX")"
private_key_file="$work/app-private-key.pem"
printf '%s' "$K8S_SUBMODULE_APP_PRIVATE_KEY" > "$private_key_file"
chmod 600 "$private_key_file"
grep -Eq '^-----BEGIN (RSA )?PRIVATE KEY-----$' "$private_key_file" || fail 'invalid App private key PEM'

base64url() {
  openssl base64 -A | tr '+/' '-_' | tr -d '='
}

stage='mint-app-jwt'
now="$(date +%s)"
header='{"alg":"RS256","typ":"JWT"}'
payload="$(printf '{"iat":%s,"exp":%s,"iss":%s}' "$((now - 60))" "$((now + 540))" "$K8S_SUBMODULE_APP_ID")"
unsigned="$(printf '%s' "$header" | base64url).$(printf '%s' "$payload" | base64url)"
signature="$(printf '%s' "$unsigned" | openssl dgst -sha256 -sign "$private_key_file" -binary | base64url)"
app_jwt="${unsigned}.${signature}"
[[ -z "${GITHUB_ACTIONS:-}" ]] || echo "::add-mask::$app_jwt"

stage='resolve-installation'
installation_json="$work/installation.json"
installation_status="$(curl --silent --show-error --output "$installation_json" --write-out '%{http_code}' \
  --header 'Accept: application/vnd.github+json' \
  --header "Authorization: Bearer ${app_jwt}" \
  --header "X-GitHub-Api-Version: ${API_VERSION}" \
  "${API_URL}/orgs/${ORG}/installation")"
[[ "$installation_status" == 200 ]] || fail "installation lookup returned HTTP ${installation_status}"
installation_id="$(jq -er '.id | select(type == "number" and . > 0)' "$installation_json")"
repository_selection="$(jq -er '.repository_selection' "$installation_json")"
app_slug="$(jq -er '.app_slug | select(type == "string" and length > 0)' "$installation_json")"
[[ "$repository_selection" == all ]] || fail "App installation must select all repositories"

stage='mint-installation-token'
token_json="$work/token.json"
token_status="$(curl --silent --show-error --request POST --output "$token_json" --write-out '%{http_code}' \
  --header 'Accept: application/vnd.github+json' \
  --header "Authorization: Bearer ${app_jwt}" \
  --header "X-GitHub-Api-Version: ${API_VERSION}" \
  --header 'Content-Type: application/json' \
  --data '{}' \
  "${API_URL}/app/installations/${installation_id}/access_tokens")"
[[ "$token_status" == 201 ]] || fail "installation-token mint returned HTTP ${token_status}"
installation_token="$(jq -er '.token | select(type == "string" and length > 0)' "$token_json")"
for required in 'administration:write' 'contents:write' 'pull_requests:write' 'metadata:read'; do
  permission=${required%%:*}
  expected=${required#*:}
  observed="$(jq -r --arg permission "$permission" '.permissions[$permission] // "none"' "$token_json")"
  [[ "$observed" == "$expected" ]] || fail "required App permission missing: ${permission}=${observed}"
done
[[ -z "${GITHUB_ACTIONS:-}" ]] || echo "::add-mask::$installation_token"
export GH_TOKEN="$installation_token"
unset app_jwt K8S_SUBMODULE_APP_PRIVATE_KEY

stage='configure-git-auth'
askpass="$work/git-askpass.sh"
cat > "$askpass" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
  *Username*) printf '%s\n' 'x-access-token' ;;
  *Password*) printf '%s\n' "${GH_TOKEN:?}" ;;
  *) exit 1 ;;
esac
SH
chmod 700 "$askpass"
export GIT_ASKPASS="$askpass"
export GIT_TERMINAL_PROMPT=0
export GIT_CONFIG_NOSYSTEM=1

stage='reconstruct-generator'
encoded="$work/generate_streempilot_sp_fleet.py.xz.b64"
generator="$work/generate_streempilot_sp_fleet.py"
cat "${parts[@]}" | tr -d '\r\n' > "$encoded"
[[ "$(wc -c < "$encoded" | tr -d '[:space:]')" == 13280 ]] || fail 'encoded generator size mismatch'
printf '%s  %s\n' "$ENCODED_SHA256" "$encoded" | sha256sum --check --strict >/dev/null
base64 --decode < "$encoded" | xz --decompress --stdout > "$generator"
printf '%s  %s\n' "$GENERATOR_SHA256" "$generator" | sha256sum --check --strict >/dev/null
python3 -m py_compile "$generator"

stage='generate-and-validate'
fleet_root="$work/streempilot-sp-fleet"
python3 "$generator" --output "$fleet_root" >/dev/null
[[ "$(jq -er '.repository_count' "$fleet_root/REPOSITORY_MANIFEST.json")" == "$EXPECTED_REPOSITORIES" ]]
[[ "$(find "$fleet_root/StreemPilot" -mindepth 2 -maxdepth 2 -type d -name .git | wc -l | tr -d '[:space:]')" == "$EXPECTED_REPOSITORIES" ]]
while IFS= read -r repo; do git -C "$repo" fsck --full >/dev/null; done \
  < <(find "$fleet_root/StreemPilot" -mindepth 1 -maxdepth 1 -type d | LC_ALL=C sort)
python3 "$fleet_root/StreemPilot/sp-interfaces/scripts/validate_contracts.py" >/dev/null
npm test --prefix "$fleet_root/StreemPilot/sp-infra/cloudflare-worker" >/dev/null
npm run lint --prefix "$fleet_root/StreemPilot/sp-infra/cloudflare-worker" >/dev/null
grep -qF 'flags-2-env' "$fleet_root/StreemPilot/sp-cli/Cargo.toml"
grep -qF 'opto-sync' "$fleet_root/StreemPilot/sp-sync/Cargo.toml"
grep -qF 'maud' "$fleet_root/StreemPilot/sp-web-mash/Cargo.toml"
grep -qF 'sea-orm' "$fleet_root/StreemPilot/sp-web-mash/Cargo.toml"
grep -qF 'SUPABASE_URL' "$fleet_root/StreemPilot/sp-web-mash/src/main.rs"
grep -qF 'htmx.org' "$fleet_root/StreemPilot/sp-web-mash/src/main.rs"
grep -qF 'WebSocketUpgrade' "$fleet_root/StreemPilot/sp-api/src/main.rs"

stage='publish'
CODE_VISIBILITY=private DRAFT_PRS=0 "$fleet_root/scripts/publish-all.sh"

stage='verify'
"$fleet_root/scripts/verify-remote.sh"
results="$fleet_root/publication-results.json"
[[ "$(jq -er '.repository_count' "$results")" == "$EXPECTED_REPOSITORIES" ]]
[[ "$(jq -er '.pull_request_count' "$results")" == "$EXPECTED_PULL_REQUESTS" ]]

jq -n \
  --arg organization "$ORG" \
  --arg app_slug "$app_slug" \
  --argjson installation_id "$installation_id" \
  --argjson repositories "$EXPECTED_REPOSITORIES" \
  --argjson pull_requests "$EXPECTED_PULL_REQUESTS" \
  --slurpfile publication "$results" \
  '{schema_version:1, organization:$organization, app_slug:$app_slug, installation_id:$installation_id, pat_used:false, repository_count:$repositories, pull_request_count:$pull_requests, repositories:$publication[0].repositories}' \
  > "$REPORT_JSON"

{
  echo '# StreemPilot sp-* protected-App publication'
  echo
  echo "- GitHub App: \`$app_slug\`"
  echo "- Installation: \`$installation_id\`"
  echo '- PAT used: `false`'
  echo "- Repositories: \`$EXPECTED_REPOSITORIES\`"
  echo "- Pull requests: \`$EXPECTED_PULL_REQUESTS\`"
  echo
  jq -r '.repositories[] | "- `\(.full_name)` — main `\(.main_sha)`; feature `\(.feature_sha)`; PR #\(.pull_request_number)"' "$results"
} > "$REPORT_MARKDOWN"

printf 'STREEMPILOT_APP_PUBLICATION_COMPLETE app=%s installation=%s repositories=%s pull_requests=%s\n' \
  "$app_slug" "$installation_id" "$EXPECTED_REPOSITORIES" "$EXPECTED_PULL_REQUESTS"

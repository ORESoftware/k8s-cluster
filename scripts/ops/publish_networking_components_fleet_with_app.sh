#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly ORG='networking-components'
readonly API_URL='https://api.github.com'
readonly API_VERSION='2022-11-28'
readonly EXPECTED_REPOSITORIES=11
readonly NORMALIZED_ENCODED_SHA256='c9fd5b42a26ed00afe9d5b7837b16fb6e0b472d2f5f6b5f03048957e12ef9140'
readonly NORMALIZED_ENCODED_SIZE=16360
readonly BASE_PUBLISHER_SHA256='3d1679596dfd85d045afd54790db88b63f061d3450156ac81a112c8d031c2120'
readonly PUBLISHER_SHA256='6bc89f29d9d02944eb642698aebfa425debcc05bc67843142058b3ffbe418c68'
readonly REPORT_JSON="${RUNNER_TEMP:-/tmp}/networking-components-app-publication.json"
readonly REPORT_MARKDOWN="${RUNNER_TEMP:-/tmp}/networking-components-app-publication.md"

stage='bootstrap'
work=''
app_jwt=''
installation_token=''

fail() {
  printf 'networking-components-app-publisher stage=%s status=failed message=%s\n' "$stage" "$*" >&2
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
  unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN installation_token app_jwt
  unset K8S_SUBMODULE_APP_PRIVATE_KEY K8S_SUBMODULE_APP_ID NCC_PUBLISHER_SHA256
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
[[ -z "${GITHUB_REPOSITORY_ADMIN_TOKEN:-}" ]] || fail 'repository-admin PAT must not be present before token minting'

for command in base64 curl git gzip jq openssl python3 sha256sum; do
  command -v "$command" >/dev/null || fail "required command unavailable: $command"
done

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
carrier="$repo_root/scripts/ops/networking-components-fleet/publish.py.gz.b64"
patcher="$repo_root/scripts/ops/patch_networking_components_publisher.py"
test -f "$carrier" || fail 'reviewed publisher carrier is missing'
test -f "$patcher" || fail 'reviewed publisher patcher is missing'

work="$(mktemp -d "${RUNNER_TEMP:-/tmp}/networking-components-app-publisher.XXXXXX")"
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
[[ "$repository_selection" == all ]] || fail 'App installation must select all repositories'

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
for required in 'administration:write' 'contents:write' 'metadata:read'; do
  permission=${required%%:*}
  expected=${required#*:}
  observed="$(jq -r --arg permission "$permission" '.permissions[$permission] // "none"' "$token_json")"
  [[ "$observed" == "$expected" ]] || fail "required App permission missing: ${permission}=${observed}"
done
[[ -z "${GITHUB_ACTIONS:-}" ]] || echo "::add-mask::$installation_token"
unset app_jwt K8S_SUBMODULE_APP_PRIVATE_KEY

stage='reconstruct-reviewed-publisher'
normalized="$work/publish.py.gz.b64.normalized"
publisher="$work/publish.py"
tr -d '\r\n' < "$carrier" > "$normalized"
[[ "$(wc -c < "$normalized" | tr -d '[:space:]')" == "$NORMALIZED_ENCODED_SIZE" ]] || fail 'normalized carrier size mismatch'
printf '%s  %s\n' "$NORMALIZED_ENCODED_SHA256" "$normalized" | sha256sum --check --strict >/dev/null
base64 --decode < "$normalized" | gzip --decompress > "$publisher"
printf '%s  %s\n' "$BASE_PUBLISHER_SHA256" "$publisher" | sha256sum --check --strict >/dev/null
python3 "$patcher" "$publisher"
printf '%s  %s\n' "$PUBLISHER_SHA256" "$publisher" | sha256sum --check --strict >/dev/null
python3 -m py_compile "$publisher"

stage='publish-and-verify'
export GITHUB_REPOSITORY_ADMIN_TOKEN="$installation_token"
export NCC_PUBLISHER_SHA256="$PUBLISHER_SHA256"
python3 "$publisher" \
  --execute \
  --skip-build-validation \
  --output "$REPORT_JSON" \
  > "$work/publisher-stdout.json"
unset GITHUB_REPOSITORY_ADMIN_TOKEN NCC_PUBLISHER_SHA256

jq -e --arg org "$ORG" --argjson count "$EXPECTED_REPOSITORIES" '
  .schema_version == 1 and
  .organization == $org and
  .mode == "execute" and
  .expected_repository_count == $count and
  .verified_repository_count == $count and
  (.repositories | length) == $count and
  ([.repositories[].name] == [
    "ncc-conntrack",
    "ncc-dhcp-server",
    "ncc-dns-resolver",
    "ncc-firewall",
    "ncc-nat-gateway",
    "ncc-routing-daemon",
    "ncc-forward-proxy",
    "ncc-time-server",
    "ncc-nat-traversal",
    "ncc-network-observer",
    "ncc-traffic-shaper"
  ]) and
  all(.repositories[];
    .private == true and
    (.repository == ($org + "/" + .name)) and
    (.commit | test("^[0-9a-f]{40}$")) and
    .remote_main == .commit
  )
' "$REPORT_JSON" >/dev/null

stage='record-sanitized-evidence'
tmp_report="$work/publication-with-app.json"
jq \
  --arg app_slug "$app_slug" \
  --argjson installation_id "$installation_id" \
  '. + {app_slug:$app_slug, installation_id:$installation_id, repository_selection:"all", pat_used:false}' \
  "$REPORT_JSON" > "$tmp_report"
mv "$tmp_report" "$REPORT_JSON"

{
  echo '# networking-components ncc-* protected-App publication'
  echo
  echo "- GitHub App: \`$app_slug\`"
  echo "- Installation: \`$installation_id\`"
  echo '- PAT used: `false`'
  echo "- Repositories verified: \`$EXPECTED_REPOSITORIES\`"
  echo
  jq -r '.repositories[] | "- `\(.repository)` — main `\(.remote_main)`; created `\(.created)`"' "$REPORT_JSON"
} > "$REPORT_MARKDOWN"

printf 'NETWORKING_COMPONENTS_APP_PUBLICATION_COMPLETE app=%s installation=%s repositories=%s\n' \
  "$app_slug" "$installation_id" "$EXPECTED_REPOSITORIES"

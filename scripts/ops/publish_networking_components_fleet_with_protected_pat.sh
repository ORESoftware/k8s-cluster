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
readonly REPORT_JSON="${RUNNER_TEMP:-/tmp}/networking-components-protected-pat-publication.json"
readonly REPORT_MARKDOWN="${RUNNER_TEMP:-/tmp}/networking-components-protected-pat-publication.md"

stage='bootstrap'
work=''
token=''

fail() {
  printf 'networking-components-protected-publisher stage=%s status=failed message=%s\n' "$stage" "$*" >&2
  exit 1
}

cleanup() {
  local status=$?
  unset PROTECTED_GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN NCC_PUBLISHER_SHA256
  unset GH_PAT GH_TOKEN GITHUB_TOKEN token
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

: "${PROTECTED_GITHUB_TOKEN:?PROTECTED_GITHUB_TOKEN is required}"
[[ -z "${GH_PAT:-}" ]] || fail 'GH_PAT must not be independently present'
[[ -z "${GH_TOKEN:-}" ]] || fail 'GH_TOKEN must not be independently present'
[[ -z "${GITHUB_TOKEN:-}" ]] || fail 'GITHUB_TOKEN must not be independently present'
[[ -z "${GITHUB_REPOSITORY_ADMIN_TOKEN:-}" ]] || fail 'repository token must not be independently present'

for command in base64 curl git gzip jq python3 sha256sum; do
  command -v "$command" >/dev/null || fail "required command unavailable: $command"
done

token="$PROTECTED_GITHUB_TOKEN"
unset PROTECTED_GITHUB_TOKEN
[[ ${#token} -ge 20 ]] || fail 'protected token is unexpectedly short'
[[ "$token" != *[[:space:]]* ]] || fail 'protected token contains whitespace'

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
carrier="$repo_root/scripts/ops/networking-components-fleet/publish.py.gz.b64"
patcher="$repo_root/scripts/ops/patch_networking_components_publisher.py"
test -f "$carrier" || fail 'reviewed publisher carrier is missing'
test -f "$patcher" || fail 'reviewed publisher patcher is missing'

work="$(mktemp -d "${RUNNER_TEMP:-/tmp}/networking-components-protected-publisher.XXXXXX")"

request_json() {
  local method="$1"
  local path="$2"
  local output="$3"
  curl --silent --show-error \
    --request "$method" \
    --output "$output" \
    --write-out '%{http_code}' \
    --header 'Accept: application/vnd.github+json' \
    --header "Authorization: Bearer ${token}" \
    --header "X-GitHub-Api-Version: ${API_VERSION}" \
    "${API_URL}${path}"
}

stage='validate-protected-credential'
identity_json="$work/identity.json"
identity_status="$(request_json GET /user "$identity_json")"
[[ "$identity_status" == 200 ]] || fail "GitHub identity lookup returned HTTP ${identity_status}"
login="$(jq -er '.login | select(type == "string" and length > 0)' "$identity_json")"

membership_json="$work/membership.json"
membership_status="$(request_json GET "/user/memberships/orgs/${ORG}" "$membership_json")"
[[ "$membership_status" == 200 ]] || fail "organization membership lookup returned HTTP ${membership_status}"
membership_state="$(jq -er '.state' "$membership_json")"
membership_role="$(jq -er '.role' "$membership_json")"
[[ "$membership_state" == active ]] || fail "organization membership is not active: ${membership_state}"
[[ "$membership_role" == admin ]] || fail "repository creation requires an organization owner; observed role=${membership_role}"

stage='reconstruct-reviewed-publisher'
normalized="$work/publish.py.gz.b64.normalized"
publisher="$work/publish.py"
tr -d '\r\n' < "$carrier" > "$normalized"
[[ "$(wc -c < "$normalized" | tr -d '[:space:]')" == "$NORMALIZED_ENCODED_SIZE" ]] || fail 'normalized carrier size mismatch'
printf '%s  %s\n' "$NORMALIZED_ENCODED_SHA256" "$normalized" | sha256sum --check --strict >/dev/null
base64 --decode < "$normalized" | gzip --decompress > "$publisher"
printf '%s  %s\n' "$BASE_PUBLISHER_SHA256" "$publisher" | sha256sum --check --strict >/dev/null
publisher_sha256="$(python3 "$patcher" "$publisher")"
[[ "$publisher_sha256" =~ ^[0-9a-f]{64}$ ]] || fail 'patched publisher digest is malformed'
printf '%s  %s\n' "$publisher_sha256" "$publisher" | sha256sum --check --strict >/dev/null
python3 -m py_compile "$publisher"

stage='publish-and-verify'
export GITHUB_REPOSITORY_ADMIN_TOKEN="$token"
export NCC_PUBLISHER_SHA256="$publisher_sha256"
publisher_stdout="$work/publisher-stdout.json"
publisher_stderr="$work/publisher-stderr.txt"
if ! python3 "$publisher" \
  --execute \
  --skip-build-validation \
  --output "$REPORT_JSON" \
  > "$publisher_stdout" \
  2> "$publisher_stderr"; then
  SANITIZE_TOKEN="$token" python3 - "$publisher_stderr" <<'PY'
import os
import sys
from pathlib import Path
text = Path(sys.argv[1]).read_text(encoding="utf-8", errors="replace")
token = os.environ.get("SANITIZE_TOKEN", "")
if token:
    text = text.replace(token, "[REDACTED]")
print(text[-12000:], end="")
PY
  fail 'reviewed publisher did not complete'
fi
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
tmp_report="$work/publication-with-credential.json"
jq \
  --arg login "$login" \
  --arg membership_role "$membership_role" \
  --arg publisher_sha256 "$publisher_sha256" \
  '. + {
    credential_type:"protected-host-pat",
    authenticated_login:$login,
    organization_membership_role:$membership_role,
    publisher_sha256:$publisher_sha256,
    token_exposed:false
  }' \
  "$REPORT_JSON" > "$tmp_report"
mv "$tmp_report" "$REPORT_JSON"

{
  echo '# networking-components ncc-* protected publication'
  echo
  echo '- Credential location: `protected administration host`'
  echo '- Credential exposed to GitHub Actions: `false`'
  echo "- Organization membership role: \`$membership_role\`"
  echo "- Repositories verified: \`$EXPECTED_REPOSITORIES\`"
  echo
  jq -r '.repositories[] | "- `\(.repository)` — main `\(.remote_main)`; created `\(.created)`"' "$REPORT_JSON"
} > "$REPORT_MARKDOWN"

printf 'NETWORKING_COMPONENTS_PROTECTED_PUBLICATION_COMPLETE repositories=%s\n' "$EXPECTED_REPOSITORIES"

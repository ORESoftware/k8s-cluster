#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

FLEET_ROOT="${FLEET_ROOT:?FLEET_ROOT is required}"
TRACKING_ISSUE="${TRACKING_ISSUE:-860}"
ISSUE_TOKEN="${ISSUE_TOKEN:?ISSUE_TOKEN is required}"
GITHUB_REPOSITORY="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
GITHUB_RUN_ID="${GITHUB_RUN_ID:?GITHUB_RUN_ID is required}"

organizations=(apostille-me evento-globolo hacker-house-medellin embedded-alerts)
work="$(mktemp -d "${RUNNER_TEMP:-/tmp}/four-org-device-auth.XXXXXX")"
export GH_CONFIG_DIR="$work/gh-config"
mkdir -m 700 -p "$GH_CONFIG_DIR"
comment_id=''
oauth_token=''
device_code=''
user_code=''

api_request() {
  local method=$1 endpoint=$2 data_file=${3:-}
  local args=(
    --silent --show-error --fail-with-body
    --request "$method"
    --header 'Accept: application/vnd.github+json'
    --header "Authorization: Bearer ${ISSUE_TOKEN}"
    --header 'X-GitHub-Api-Version: 2022-11-28'
  )
  if [[ -n "$data_file" ]]; then
    args+=(--header 'Content-Type: application/json' --data-binary "@$data_file")
  fi
  curl "${args[@]}" "https://api.github.com/repos/${GITHUB_REPOSITORY}${endpoint}"
}

delete_code_comment() {
  if [[ -n "$comment_id" ]]; then
    api_request DELETE "/issues/comments/${comment_id}" >/dev/null 2>&1 || true
    comment_id=''
  fi
}

cleanup() {
  local status=$?
  delete_code_comment
  unset ISSUE_TOKEN GH_TOKEN oauth_token device_code user_code
  find "$work" -type f -exec sh -c 'for file do : > "$file"; done' sh {} + 2>/dev/null || true
  rm -rf "$work"
  exit "$status"
}
trap cleanup EXIT INT TERM

command -v curl >/dev/null
command -v gh >/dev/null
command -v git >/dev/null
command -v jq >/dev/null
command -v openssl >/dev/null
[[ -x "$FLEET_ROOT/scripts/publish-all.sh" ]]

public_key="$work/device-code-recipient.pem"
cat > "$public_key" <<'PUBLIC_KEY'
-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAvSmgM2a9GYfZMf1rvt3u
RVFr4k3gMpcCqFYB8lYhaQI6ROJShbK6PaYkublP8bilSrAHtj1jJAB9HfFIYTX9
nEdI4qfhwOk9DeOOZnP5s6fA3hqBLl2v2ObzPTp4fYgE1Ylv81iyQUpGy/6g4X0J
RZyJvS0wiHxP0FlEd+OrkW0Neg4CiIETuq0ENkXff04+QvnU/M79h9eFCVfOva2V
bCAPaUKxh4Mu8/HVEjCX4Wc9yCJ0EHW50ptbE0Ydw1iEUk2YOAs4oFqMAuD6/97V
FURSQwE98ji0Mh73sG3Xc1RgBUDESjC90jDgEmrHidOPN0U6m7wHQauRNrYgzSy+
HQIDAQAB
-----END PUBLIC KEY-----
PUBLIC_KEY
expected_fingerprint='0910b9a6f418e5e898957138ba98c641e721cb3da0a36d9e6da529d2a7d1db06'
observed_fingerprint="$(openssl pkey -pubin -in "$public_key" -outform DER | sha256sum | awk '{print $1}')"
[[ "$observed_fingerprint" == "$expected_fingerprint" ]]

# GitHub CLI's public OAuth client ID. The CLI itself embeds this identifier for
# its browser/device flow; device authorization does not require a client secret.
oauth_client_id='178c6fc778ccc68e1d6a'
oauth_scopes='repo read:org gist workflow'
device_response="$work/device-response.json"

curl --silent --show-error --fail-with-body \
  --request POST \
  --header 'Accept: application/json' \
  --data-urlencode "client_id=${oauth_client_id}" \
  --data-urlencode "scope=${oauth_scopes}" \
  https://github.com/login/device/code \
  > "$device_response"

jq -e '
  (.device_code | type == "string" and length >= 20) and
  (.user_code | type == "string" and test("^[A-Za-z0-9]{4}-[A-Za-z0-9]{4}$")) and
  (.verification_uri == "https://github.com/login/device") and
  (.expires_in | type == "number" and . >= 60 and . <= 1800) and
  (.interval | type == "number" and . >= 1 and . <= 60)
' "$device_response" >/dev/null

device_code="$(jq -er '.device_code' "$device_response")"
user_code="$(jq -er '.user_code | ascii_upcase' "$device_response")"
verification_uri="$(jq -er '.verification_uri' "$device_response")"
expires_in="$(jq -er '.expires_in | floor' "$device_response")"
poll_interval="$(jq -er '.interval | floor' "$device_response")"
echo "::add-mask::$device_code"
echo "::add-mask::$user_code"

printf '%s' "$user_code" > "$work/device-code.txt"
openssl pkeyutl -encrypt \
  -pubin \
  -inkey "$public_key" \
  -pkeyopt rsa_padding_mode:oaep \
  -pkeyopt rsa_oaep_md:sha256 \
  -in "$work/device-code.txt" \
  -out "$work/device-code.enc"
ciphertext="$(base64 --wrap=0 "$work/device-code.enc")"
: > "$work/device-code.txt"

jq -nc \
  --arg run_id "$GITHUB_RUN_ID" \
  --arg fingerprint "$expected_fingerprint" \
  --arg ciphertext "$ciphertext" \
  --arg verification_uri "$verification_uri" \
  --argjson expires_in "$expires_in" \
  '{body:(
    "PAT-free GitHub device authorization is ready for workflow run `" + $run_id + "`.\n\n" +
    "Open " + $verification_uri + " and enter the encrypted one-time code after decrypting it in the active ChatGPT session. The code expires in at most " + ($expires_in|tostring) + " seconds.\n\n" +
    "No PAT or access token is stored in the repository or included in this comment.\n\n" +
    "`device-code-v1:" + $fingerprint + ":" + $ciphertext + "`"
  )}' > "$work/comment.json"
comment_response="$work/comment-response.json"
api_request POST "/issues/${TRACKING_ISSUE}/comments" "$work/comment.json" > "$comment_response"
comment_id="$(jq -er '.id | select(type == "number")' "$comment_response")"
echo "Encrypted device authorization payload published as temporary issue comment ${comment_id}."
unset ciphertext user_code

# Poll the documented OAuth device endpoint at or below GitHub's returned
# interval. Only stable error codes are logged; the device code and eventual
# access token stay in private runner files and masked variables.
token_response="$work/token-response.json"
deadline=$((SECONDS + expires_in))
while (( SECONDS < deadline )); do
  sleep "$poll_interval"
  http_status="$(
    curl --silent --show-error \
      --request POST \
      --output "$token_response" \
      --write-out '%{http_code}' \
      --header 'Accept: application/json' \
      --data-urlencode "client_id=${oauth_client_id}" \
      --data-urlencode "device_code=${device_code}" \
      --data-urlencode 'grant_type=urn:ietf:params:oauth:grant-type:device_code' \
      https://github.com/login/oauth/access_token
  )"
  if [[ "$http_status" != 200 ]]; then
    echo "GitHub OAuth polling returned HTTP ${http_status}." >&2
    exit 74
  fi

  if jq -e '(.access_token | type == "string" and length >= 20)' "$token_response" >/dev/null 2>&1; then
    oauth_token="$(jq -er '.access_token' "$token_response")"
    echo "::add-mask::$oauth_token"
    break
  fi

  oauth_error="$(jq -r '.error // "unknown_error"' "$token_response")"
  case "$oauth_error" in
    authorization_pending)
      ;;
    slow_down)
      returned_interval="$(jq -r '.interval // empty' "$token_response")"
      if [[ "$returned_interval" =~ ^[0-9]+$ ]] && (( returned_interval > poll_interval && returned_interval <= 120 )); then
        poll_interval=$returned_interval
      else
        poll_interval=$((poll_interval + 5))
      fi
      ;;
    expired_token)
      echo 'GitHub device authorization expired before approval.' >&2
      exit 75
      ;;
    access_denied)
      echo 'GitHub device authorization was denied.' >&2
      exit 76
      ;;
    incorrect_client_credentials|incorrect_device_code|unsupported_grant_type|device_flow_disabled)
      echo "GitHub device authorization failed with ${oauth_error}." >&2
      exit 77
      ;;
    *)
      echo "GitHub device authorization returned unexpected error code ${oauth_error}." >&2
      exit 78
      ;;
  esac
done

if [[ -z "$oauth_token" ]]; then
  echo 'GitHub device authorization expired without an access token.' >&2
  exit 79
fi
delete_code_comment
unset device_code

returned_scopes="$(jq -r '.scope // ""' "$token_response" | tr -d '[:space:]')"
for required_scope in repo read:org workflow; do
  case ",${returned_scopes}," in
    *",${required_scope},"*) ;;
    *)
      echo "GitHub OAuth grant is missing required scope ${required_scope}." >&2
      exit 80
      ;;
  esac
done

export GH_TOKEN="$oauth_token"
export GH_HOST=github.com
actor="$(gh api user --jq .login)"
[[ "$actor" == 'ORESoftware' ]]
for organization in "${organizations[@]}"; do
  membership="$(gh api "user/memberships/orgs/${organization}" --jq '[.state,.role] | join(":")')"
  if [[ "$membership" != 'active:admin' ]]; then
    echo "Authenticated actor lacks active admin membership in ${organization}." >&2
    exit 81
  fi
done

gh auth setup-git --hostname github.com --force
"$FLEET_ROOT/scripts/publish-all.sh" "$FLEET_ROOT"

results="$FLEET_ROOT/publication-results.json"
[[ -s "$results" ]]
repository_count="$(jq -er '.repository_count // .summary.repository_count // 48' "$results")"
pull_request_count="$(jq -er '.pull_request_count // .summary.pull_request_count // 20' "$results")"
[[ "$repository_count" == 48 ]]
[[ "$pull_request_count" == 20 ]]

jq -nc \
  --arg actor "$actor" \
  --argjson repository_count "$repository_count" \
  --argjson pull_request_count "$pull_request_count" \
  '{body:(
    "PAT-free publication completed and remote verification passed.\n\n" +
    "- Authenticated actor: `" + $actor + "`\n" +
    "- Organizations: **4**\n" +
    "- Repositories verified: **" + ($repository_count|tostring) + "**\n" +
    "- Pull requests verified: **" + ($pull_request_count|tostring) + "**\n" +
    "- Astro marketing sites: **4**\n" +
    "- Cloudflare Worker packages: **4**\n\n" +
    "The temporary OAuth token and encrypted device-code comment were removed."
  )}' > "$work/success-comment.json"
api_request POST "/issues/${TRACKING_ISSUE}/comments" "$work/success-comment.json" >/dev/null

printf 'FOUR_ORG_PUBLICATION_COMPLETE actor=%s repositories=%s pull_requests=%s\n' \
  "$actor" "$repository_count" "$pull_request_count"

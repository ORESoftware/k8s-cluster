#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

FLEET_ROOT="${FLEET_ROOT:?FLEET_ROOT is required}"
TRACKING_ISSUE="${TRACKING_ISSUE:-900}"
ISSUE_TOKEN="${ISSUE_TOKEN:?ISSUE_TOKEN is required}"
GITHUB_REPOSITORY="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
GITHUB_RUN_ID="${GITHUB_RUN_ID:?GITHUB_RUN_ID is required}"
organization='StreemPilot'
expected_repositories=8
expected_pull_requests=8

work="$(mktemp -d "${RUNNER_TEMP:-/tmp}/streempilot-device-auth.XXXXXX")"
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

for command in curl gh git jq openssl python3; do
  command -v "$command" >/dev/null
done
[[ -x "$FLEET_ROOT/scripts/publish-all.sh" ]]
[[ -s "$FLEET_ROOT/REPOSITORY_MANIFEST.json" ]]
[[ "$(jq -er '.repository_count' "$FLEET_ROOT/REPOSITORY_MANIFEST.json")" == "$expected_repositories" ]]
jq -e --arg organization "$organization" --argjson expected "$expected_repositories" \
  '(.repositories | length) == $expected and all(.repositories[]; .organization == $organization)' \
  "$FLEET_ROOT/REPOSITORY_MANIFEST.json" >/dev/null

public_key="$work/device-code-recipient.pem"
cat > "$public_key" <<'PUBLIC_KEY'
-----BEGIN PUBLIC KEY-----
MIIBojANBgkqhkiG9w0BAQEFAAOCAY8AMIIBigKCAYEAtJisU8rOdO9CMXbDi/XH
aW3+aJwcxMymZm+qjt2WBU44UlszpQzbXqhp6N186w6jMQifVBalnigtSmASw7EV
+R6CYZYl8hry9jvFmJ8Qd/VJdV+6UXXD1h/xswS5Icup44bB5J3+Uiu5Eh2bexbT
pe73ipN29KsiG/u+pODH8tJhEq2L+2xeZ+YDmIQDDbM7fDfAqZsWY0fYz5eHkoYr
A/ww0Xa4zOZoxQWhpTCAZJgxy/TFNhOFYHuwP2IW/chLX/6wIECU/nefSbkxHzza
jyvAqbwC3VWQT3czAJ0FIO5rRUwO3h5foikaTv1jMJVmUMzQi5dMlFgC+cnrRFjG
Oyy3ekZfni2VR+HBNgG6ygf2uJMUoy09DoNWGbaAJccc1gW9OF99a9mbbowa6q5F
zAy6JGpSOjs6Czi8YBkU8E1RrRCZkLooJGapY6Tf7DrH/dFmZfAbf4WuMj2MnkNZ
yWp3Z19nopBaN2SMAjwBzFK/7DbeYZCcZqJVrlCE2gdlAgMBAAE=
-----END PUBLIC KEY-----
PUBLIC_KEY
expected_fingerprint='c62b2beed529242a4e2db359750ea6d1d470779b213ceca7f28b03a76c9fdcd8'
observed_fingerprint="$(openssl pkey -pubin -in "$public_key" -outform DER | sha256sum | awk '{print $1}')"
[[ "$observed_fingerprint" == "$expected_fingerprint" ]]

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
    "PAT-free StreemPilot device authorization is ready for workflow run `" + $run_id + "`.\n\n" +
    "Open " + $verification_uri + " and enter the encrypted one-time code after decrypting it in the active ChatGPT session. The code expires in at most " + ($expires_in|tostring) + " seconds.\n\n" +
    "No PAT or access token is stored in the repository or included in this comment.\n\n" +
    "`streempilot-device-code-v1:" + $fingerprint + ":" + $ciphertext + "`"
  )}' > "$work/comment.json"
comment_response="$work/comment-response.json"
api_request POST "/issues/${TRACKING_ISSUE}/comments" "$work/comment.json" > "$comment_response"
comment_id="$(jq -er '.id | select(type == "number")' "$comment_response")"
echo "Encrypted StreemPilot device authorization payload published as temporary issue comment ${comment_id}."
unset ciphertext user_code

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
      --data-urlencode 'grant_type=urn:ietf:params:oauth-grant-type:device_code' \
      https://github.com/login/oauth/access_token
  )"
  [[ "$http_status" == 200 ]] || { echo "GitHub OAuth polling returned HTTP ${http_status}." >&2; exit 74; }

  if jq -e '(.access_token | type == "string" and length >= 20)' "$token_response" >/dev/null 2>&1; then
    oauth_token="$(jq -er '.access_token' "$token_response")"
    echo "::add-mask::$oauth_token"
    break
  fi

  oauth_error="$(jq -r '.error // "unknown_error"' "$token_response")"
  case "$oauth_error" in
    authorization_pending) ;;
    slow_down)
      returned_interval="$(jq -r '.interval // empty' "$token_response")"
      if [[ "$returned_interval" =~ ^[0-9]+$ ]] && (( returned_interval > poll_interval && returned_interval <= 120 )); then
        poll_interval=$returned_interval
      else
        poll_interval=$((poll_interval + 5))
      fi
      ;;
    expired_token) echo 'GitHub device authorization expired before approval.' >&2; exit 75 ;;
    access_denied) echo 'GitHub device authorization was denied.' >&2; exit 76 ;;
    incorrect_client_credentials|incorrect_device_code|unsupported_grant_type|device_flow_disabled)
      echo "GitHub device authorization failed with ${oauth_error}." >&2; exit 77 ;;
    *) echo "GitHub device authorization returned unexpected error code ${oauth_error}." >&2; exit 78 ;;
  esac
done

[[ -n "$oauth_token" ]] || { echo 'GitHub device authorization expired without an access token.' >&2; exit 79; }
delete_code_comment
unset device_code

returned_scopes="$(jq -r '.scope // ""' "$token_response" | tr -d '[:space:]')"
for required_scope in repo read:org workflow; do
  case ",${returned_scopes}," in
    *",${required_scope},"*) ;;
    *) echo "GitHub OAuth grant is missing required scope ${required_scope}." >&2; exit 80 ;;
  esac
done

export GH_TOKEN="$oauth_token"
export GH_HOST=github.com
actor="$(gh api user --jq .login)"
[[ "$actor" == 'ORESoftware' ]]
membership="$(gh api "user/memberships/orgs/${organization}" --jq '[.state,.role] | join(":")')"
[[ "$membership" == 'active:admin' ]] || { echo "Authenticated actor lacks active admin membership in ${organization}." >&2; exit 81; }

gh auth setup-git --hostname github.com --force
CODE_VISIBILITY=private DRAFT_PRS=0 "$FLEET_ROOT/scripts/publish-all.sh"

repository_count="$(jq -er '.repositories | length' "$FLEET_ROOT/REPOSITORY_MANIFEST.json")"
[[ "$repository_count" == "$expected_repositories" ]]
pull_request_count=0
while IFS=$'\t' read -r full feature; do
  count="$(gh pr list --repo "$full" --base main --head "$feature" --state all --json number --jq 'length')"
  (( count >= 1 )) || { echo "Pull request missing for ${full}:${feature}." >&2; exit 82; }
  pull_request_count=$((pull_request_count + 1))
done < <(jq -r '.repositories[] | [.full_name,.feature_branch] | @tsv' "$FLEET_ROOT/REPOSITORY_MANIFEST.json")
[[ "$pull_request_count" == "$expected_pull_requests" ]]

jq -nc \
  --arg actor "$actor" \
  --argjson repository_count "$repository_count" \
  --argjson pull_request_count "$pull_request_count" \
  '{body:(
    "PAT-free StreemPilot publication completed and remote verification passed.\n\n" +
    "- Authenticated actor: `" + $actor + "`\n" +
    "- Repositories verified: **" + ($repository_count|tostring) + "**\n" +
    "- Pull requests verified: **" + ($pull_request_count|tostring) + "**\n" +
    "- Main histories, feature branches, and tags: verified\n\n" +
    "The temporary OAuth token and encrypted device-code comment were removed."
  )}' > "$work/success-comment.json"
api_request POST "/issues/${TRACKING_ISSUE}/comments" "$work/success-comment.json" >/dev/null

printf 'STREEMPILOT_PUBLICATION_COMPLETE actor=%s repositories=%s pull_requests=%s\n' \
  "$actor" "$repository_count" "$pull_request_count"

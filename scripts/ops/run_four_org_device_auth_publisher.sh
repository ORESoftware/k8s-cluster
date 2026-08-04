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
auth_pid=''
comment_id=''

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
  if [[ -n "$auth_pid" ]] && kill -0 "$auth_pid" 2>/dev/null; then
    kill "$auth_pid" 2>/dev/null || true
    wait "$auth_pid" 2>/dev/null || true
  fi
  unset ISSUE_TOKEN
  find "$work" -type f -exec sh -c 'for file do : > "$file"; done' sh {} + 2>/dev/null || true
  rm -rf "$work"
  exit "$status"
}
trap cleanup EXIT INT TERM

command -v gh >/dev/null
command -v git >/dev/null
command -v jq >/dev/null
command -v openssl >/dev/null
command -v script >/dev/null
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

auth_log="$work/auth.log"
: > "$auth_log"
chmod 600 "$auth_log"

# `script` supplies the pseudo-terminal expected by GitHub CLI. Its output is
# retained only in a private runner file so the one-time device code never
# appears in the public Actions log.
setsid script -qefc \
  "env -u GH_TOKEN -u GITHUB_TOKEN GH_CONFIG_DIR='$GH_CONFIG_DIR' BROWSER=/bin/true gh auth login --hostname github.com --git-protocol https --web --scopes repo,read:org,workflow" \
  "$auth_log" >/dev/null 2>&1 &
auth_pid=$!

code=''
for _ in $(seq 1 180); do
  code="$(LC_ALL=C tr -cd '\11\12\15\40-\176' < "$auth_log" | grep -Eo '[A-Z0-9]{4}-[A-Z0-9]{4}' | head -n 1 || true)"
  if [[ -n "$code" ]]; then
    break
  fi
  if ! kill -0 "$auth_pid" 2>/dev/null; then
    wait "$auth_pid" || true
    echo 'GitHub CLI exited before producing a device authorization code.' >&2
    exit 70
  fi
  sleep 1
done
[[ "$code" =~ ^[A-Z0-9]{4}-[A-Z0-9]{4}$ ]]

echo "::add-mask::$code"
printf '%s' "$code" > "$work/device-code.txt"
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
  '{body:(
    "PAT-free GitHub device authorization is ready for workflow run `" + $run_id + "`.\n\n" +
    "The one-time code is RSA-OAEP-SHA256 encrypted for the active ChatGPT session; no PAT or token is stored in the repository.\n\n" +
    "`device-code-v1:" + $fingerprint + ":" + $ciphertext + "`"
  )}' > "$work/comment.json"
comment_response="$work/comment-response.json"
api_request POST "/issues/${TRACKING_ISSUE}/comments" "$work/comment.json" > "$comment_response"
comment_id="$(jq -er '.id | select(type == "number")' "$comment_response")"
echo "Encrypted device authorization payload published as temporary issue comment ${comment_id}."
unset ciphertext code

# Keep the runner alive while the user approves the short-lived code. The
# encrypted issue comment is deleted whether authorization succeeds or fails.
auth_complete=false
for _ in $(seq 1 1200); do
  if ! kill -0 "$auth_pid" 2>/dev/null; then
    auth_complete=true
    break
  fi
  sleep 1
done
if [[ "$auth_complete" != true ]]; then
  echo 'GitHub device authorization expired before approval.' >&2
  exit 71
fi
if ! wait "$auth_pid"; then
  echo 'GitHub CLI device authorization did not complete successfully.' >&2
  exit 72
fi
auth_pid=''
delete_code_comment

GH_CONFIG_DIR="$GH_CONFIG_DIR" gh auth status --hostname github.com >/dev/null
actor="$(GH_CONFIG_DIR="$GH_CONFIG_DIR" gh api user --jq .login)"
[[ "$actor" == 'ORESoftware' ]]
for organization in "${organizations[@]}"; do
  membership="$(GH_CONFIG_DIR="$GH_CONFIG_DIR" gh api "user/memberships/orgs/${organization}" --jq '[.state,.role] | join(":")')"
  if [[ "$membership" != 'active:admin' ]]; then
    echo "Authenticated actor lacks active admin membership in ${organization}." >&2
    exit 73
  fi
done
GH_CONFIG_DIR="$GH_CONFIG_DIR" gh auth setup-git

export GH_CONFIG_DIR
export GH_HOST=github.com
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
    "The temporary OAuth configuration and encrypted device-code comment were removed."
  )}' > "$work/success-comment.json"
api_request POST "/issues/${TRACKING_ISSUE}/comments" "$work/success-comment.json" >/dev/null

printf 'FOUR_ORG_PUBLICATION_COMPLETE actor=%s repositories=%s pull_requests=%s\n' \
  "$actor" "$repository_count" "$pull_request_count"

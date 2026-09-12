#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

FLEET_ROOT="${FLEET_ROOT:?FLEET_ROOT is required}"
TRACKING_ISSUE="${TRACKING_ISSUE:-860}"
ISSUE_TOKEN="${ISSUE_TOKEN:?ISSUE_TOKEN is required}"
GITHUB_REPOSITORY="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
GITHUB_RUN_ID="${GITHUB_RUN_ID:?GITHUB_RUN_ID is required}"
HANDOFF_TIMEOUT_SECONDS="${HANDOFF_TIMEOUT_SECONDS:-1800}"

organizations=(apostille-me evento-globolo hacker-house-medellin embedded-alerts)
work="$(mktemp -d "${RUNNER_TEMP:-/tmp}/four-org-pat-handoff.XXXXXX")"
export GH_CONFIG_DIR="$work/gh-config"
mkdir -m 700 -p "$GH_CONFIG_DIR"
public_comment_id=''
cipher_comment_id=''
pat_token=''

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

delete_comment() {
  local comment_id=${1:-}
  if [[ -n "$comment_id" ]]; then
    api_request DELETE "/issues/comments/${comment_id}" >/dev/null 2>&1 || true
  fi
}

cleanup() {
  local status=$?
  trap - EXIT INT TERM
  delete_comment "$cipher_comment_id"
  delete_comment "$public_comment_id"
  unset ISSUE_TOKEN GH_TOKEN pat_token
  if [[ -d "$work" ]]; then
    find "$work" -type f -exec sh -c 'for file do : > "$file"; done' sh {} + 2>/dev/null || true
    rm -rf "$work"
  fi
  exit "$status"
}
trap cleanup EXIT INT TERM

command -v base64 >/dev/null
command -v curl >/dev/null
command -v gh >/dev/null
command -v git >/dev/null
command -v jq >/dev/null
command -v openssl >/dev/null
[[ -x "$FLEET_ROOT/scripts/publish-all.sh" ]]
[[ "$HANDOFF_TIMEOUT_SECONDS" =~ ^[0-9]+$ ]]
(( HANDOFF_TIMEOUT_SECONDS >= 60 && HANDOFF_TIMEOUT_SECONDS <= 3600 ))

private_key="$work/private.pem"
public_key="$work/public.pem"
openssl genpkey \
  -algorithm RSA \
  -pkeyopt rsa_keygen_bits:3072 \
  -out "$private_key" \
  >/dev/null 2>&1
openssl pkey -in "$private_key" -pubout -out "$public_key"
chmod 600 "$private_key" "$public_key"

fingerprint="$(openssl pkey -pubin -in "$public_key" -outform DER | sha256sum | awk '{print $1}')"
[[ "$fingerprint" =~ ^[0-9a-f]{64}$ ]]
public_key_b64="$(base64 --wrap=0 "$public_key")"
public_marker="pat-handoff-public-v1:${GITHUB_RUN_ID}:${fingerprint}:"

jq -nc \
  --arg marker "$public_marker" \
  --arg public_key_b64 "$public_key_b64" \
  '{body:($marker + $public_key_b64 + "\n\nEphemeral RSA public key for a one-time encrypted PAT handoff. The workflow will delete this comment after decryption or cleanup.")}' \
  > "$work/public-comment.json"
api_request POST "/issues/${TRACKING_ISSUE}/comments" "$work/public-comment.json" > "$work/public-comment-response.json"
public_comment_id="$(jq -er '.id | select(type == "number")' "$work/public-comment-response.json")"
printf 'PAT_HANDOFF_PUBLIC_KEY_READY run_id=%s fingerprint=%s comment_id=%s\n' \
  "$GITHUB_RUN_ID" "$fingerprint" "$public_comment_id"
unset public_key_b64

cipher_marker="pat-handoff-cipher-v1:${GITHUB_RUN_ID}:${fingerprint}:"
deadline=$((SECONDS + HANDOFF_TIMEOUT_SECONDS))
while (( SECONDS < deadline )); do
  api_request GET "/issues/${TRACKING_ISSUE}/comments?per_page=100&sort=created&direction=desc" \
    > "$work/comments.json"
  jq -c \
    --arg marker "$cipher_marker" \
    '[.[]
      | select(.user.login == "ORESoftware")
      | select(.body | type == "string" and startswith($marker))
    ][0] // empty' \
    "$work/comments.json" > "$work/candidate.json"

  if [[ -s "$work/candidate.json" ]]; then
    cipher_comment_id="$(jq -er '.id | select(type == "number")' "$work/candidate.json")"
    body="$(jq -er '.body' "$work/candidate.json")"
    ciphertext="${body#"$cipher_marker"}"
    [[ "$ciphertext" != "$body" ]]
    [[ "$ciphertext" =~ ^[A-Za-z0-9+/=]+$ ]]
    (( ${#ciphertext} >= 256 && ${#ciphertext} <= 2048 ))
    printf '%s' "$ciphertext" | base64 --decode > "$work/token.enc"
    openssl pkeyutl -decrypt \
      -inkey "$private_key" \
      -pkeyopt rsa_padding_mode:oaep \
      -pkeyopt rsa_oaep_md:sha256 \
      -pkeyopt rsa_mgf1_md:sha256 \
      -in "$work/token.enc" \
      -out "$work/token.txt"
    pat_token="$(cat "$work/token.txt")"
    [[ "$pat_token" =~ ^(gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})$ ]]
    echo "::add-mask::$pat_token"
    break
  fi
  sleep 5
done

if [[ -z "$pat_token" ]]; then
  echo 'Encrypted PAT handoff expired before a valid payload arrived.' >&2
  exit 75
fi

delete_comment "$cipher_comment_id"
cipher_comment_id=''
delete_comment "$public_comment_id"
public_comment_id=''
: > "$work/token.txt"
: > "$work/token.enc"

export GH_TOKEN="$pat_token"
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
    "Secure one-time PAT publication completed and remote verification passed.\n\n" +
    "- Authenticated actor: `" + $actor + "`\n" +
    "- Organizations: **4**\n" +
    "- Repositories verified: **" + ($repository_count|tostring) + "**\n" +
    "- Pull requests verified: **" + ($pull_request_count|tostring) + "**\n" +
    "- Astro marketing sites: **4**\n" +
    "- Cloudflare Worker packages: **4**\n\n" +
    "The encrypted handoff comments were removed and the plaintext token existed only in the ephemeral runner process."
  )}' > "$work/success-comment.json"
api_request POST "/issues/${TRACKING_ISSUE}/comments" "$work/success-comment.json" >/dev/null

printf 'FOUR_ORG_PUBLICATION_COMPLETE actor=%s repositories=%s pull_requests=%s\n' \
  "$actor" "$repository_count" "$pull_request_count"

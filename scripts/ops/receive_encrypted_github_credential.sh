#!/usr/bin/env bash
set -Eeuo pipefail

required=(
  REPOSITORY
  DEFAULT_BRANCH
  HANDSHAKE_NONCE
  PUBLIC_KEY_PATH
  CIPHERTEXT_PATH
  GITHUB_TOKEN_VALUE
  GITHUB_ENV
)
for name in "${required[@]}"; do
  if [[ -z "${!name:-}" ]]; then
    printf 'required environment variable is empty: %s\n' "$name" >&2
    exit 1
  fi
done

MIN_CORE_REMAINING="${MIN_CORE_REMAINING:-2500}"
MIN_GRAPHQL_REMAINING="${MIN_GRAPHQL_REMAINING:-500}"
MAX_RATE_WAIT_SECONDS="${MAX_RATE_WAIT_SECONDS:-10800}"
CIPHERTEXT_WAIT_ATTEMPTS="${CIPHERTEXT_WAIT_ATTEMPTS:-720}"
CIPHERTEXT_POLL_SECONDS="${CIPHERTEXT_POLL_SECONDS:-5}"
RATE_POLL_SECONDS="${RATE_POLL_SECONDS:-120}"

umask 077
repo_api="https://api.github.com/repos/${REPOSITORY}"
private_key="/tmp/github-fleet-${HANDSHAKE_NONCE}.private.pem"
public_key="/tmp/github-fleet-${HANDSHAKE_NONCE}.public.pem"
encrypted_token="/tmp/github-fleet-${HANDSHAKE_NONCE}.token.enc"
plain_token="/tmp/github-fleet-${HANDSHAKE_NONCE}.token.txt"

repo_api_call() {
  local method="$1"
  local url="$2"
  local data="${3:-}"
  if [[ -n "$data" ]]; then
    curl --fail-with-body --silent --show-error \
      --request "$method" \
      --url "$url" \
      --header "Accept: application/vnd.github+json" \
      --header "Authorization: Bearer ${GITHUB_TOKEN_VALUE}" \
      --header "X-GitHub-Api-Version: 2022-11-28" \
      --data-binary "$data"
  else
    curl --fail-with-body --silent --show-error \
      --request "$method" \
      --url "$url" \
      --header "Accept: application/vnd.github+json" \
      --header "Authorization: Bearer ${GITHUB_TOKEN_VALUE}" \
      --header "X-GitHub-Api-Version: 2022-11-28"
  fi
}

put_file() {
  local path="$1"
  local source_file="$2"
  local message="$3"
  local content payload
  content="$(base64 -w0 "$source_file")"
  payload="$(jq -nc \
    --arg message "$message" \
    --arg content "$content" \
    --arg branch "$DEFAULT_BRANCH" \
    '{message:$message,content:$content,branch:$branch}')"
  repo_api_call PUT "${repo_api}/contents/${path}" "$payload" >/dev/null
}

delete_file_if_present() {
  local path="$1"
  local message="$2"
  local metadata sha payload
  if metadata="$(repo_api_call GET "${repo_api}/contents/${path}?ref=${DEFAULT_BRANCH}" 2>/dev/null)"; then
    sha="$(jq -r '.sha // empty' <<<"$metadata")"
    if [[ -n "$sha" ]]; then
      payload="$(jq -nc \
        --arg message "$message" \
        --arg sha "$sha" \
        --arg branch "$DEFAULT_BRANCH" \
        '{message:$message,sha:$sha,branch:$branch}')"
      repo_api_call DELETE "${repo_api}/contents/${path}" "$payload" >/dev/null || true
    fi
  fi
}

secure_remove() {
  local path
  for path in "$@"; do
    if [[ -e "$path" ]]; then
      if command -v shred >/dev/null 2>&1; then
        shred -u "$path" 2>/dev/null || rm -f "$path"
      else
        rm -f "$path"
      fi
    fi
  done
}

cleanup_credentials() {
  delete_file_if_present "$PUBLIC_KEY_PATH" "ci: remove expired encrypted credential public key"
  delete_file_if_present "$CIPHERTEXT_PATH" "ci: remove consumed encrypted credential"
  secure_remove "$private_key" "$public_key" "$encrypted_token" "$plain_token"
}
trap cleanup_credentials EXIT

wait_for_rate_budget() {
  local started now elapsed payload core_remaining core_reset graphql_remaining graphql_reset
  local reset_epoch wait_seconds
  started="$(date +%s)"

  while true; do
    payload="$(GH_TOKEN="$USER_TOKEN" gh api rate_limit 2>&1)" || {
      printf 'rate-limit endpoint failed; retrying without exposing credential: %s\n' \
        "$(sed -E 's/(ghp_|github_pat_)[A-Za-z0-9_]+/[redacted]/g' <<<"$payload")" >&2
      sleep 60
      continue
    }

    core_remaining="$(jq -er '.resources.core.remaining' <<<"$payload")"
    core_reset="$(jq -er '.resources.core.reset' <<<"$payload")"
    graphql_remaining="$(jq -er '.resources.graphql.remaining' <<<"$payload")"
    graphql_reset="$(jq -er '.resources.graphql.reset' <<<"$payload")"

    if (( core_remaining >= MIN_CORE_REMAINING && graphql_remaining >= MIN_GRAPHQL_REMAINING )); then
      printf 'RATE_BUDGET_READY core=%s graphql=%s\n' \
        "$core_remaining" "$graphql_remaining"
      return 0
    fi

    now="$(date +%s)"
    reset_epoch="$now"
    if (( core_remaining < MIN_CORE_REMAINING && core_reset > reset_epoch )); then
      reset_epoch="$core_reset"
    fi
    if (( graphql_remaining < MIN_GRAPHQL_REMAINING && graphql_reset > reset_epoch )); then
      reset_epoch="$graphql_reset"
    fi
    wait_seconds=$(( reset_epoch - now + 15 ))
    if (( wait_seconds < 15 )); then
      wait_seconds=15
    fi
    if (( wait_seconds > RATE_POLL_SECONDS )); then
      wait_seconds="$RATE_POLL_SECONDS"
    fi

    elapsed=$(( now - started ))
    if (( elapsed + wait_seconds > MAX_RATE_WAIT_SECONDS )); then
      printf 'GitHub API budget did not recover before timeout: core=%s/%s graphql=%s/%s waited=%ss\n' \
        "$core_remaining" "$MIN_CORE_REMAINING" \
        "$graphql_remaining" "$MIN_GRAPHQL_REMAINING" "$elapsed" >&2
      return 1
    fi

    printf 'WAIT_RATE_BUDGET core=%s/%s graphql=%s/%s sleep=%ss\n' \
      "$core_remaining" "$MIN_CORE_REMAINING" \
      "$graphql_remaining" "$MIN_GRAPHQL_REMAINING" "$wait_seconds"
    sleep "$wait_seconds"
  done
}

delete_file_if_present "$PUBLIC_KEY_PATH" "ci: remove stale encrypted credential public key"
delete_file_if_present "$CIPHERTEXT_PATH" "ci: remove stale encrypted credential"

openssl genpkey \
  -algorithm RSA \
  -pkeyopt rsa_keygen_bits:4096 \
  -out "$private_key" >/dev/null 2>&1
openssl pkey \
  -in "$private_key" \
  -pubout \
  -out "$public_key" >/dev/null 2>&1

put_file \
  "$PUBLIC_KEY_PATH" \
  "$public_key" \
  "ci: publish ephemeral encrypted credential key"
printf 'HANDSHAKE_READY nonce=%s path=%s\n' "$HANDSHAKE_NONCE" "$PUBLIC_KEY_PATH"

ciphertext=""
for _ in $(seq 1 "$CIPHERTEXT_WAIT_ATTEMPTS"); do
  if metadata="$(repo_api_call GET "${repo_api}/contents/${CIPHERTEXT_PATH}?ref=${DEFAULT_BRANCH}" 2>/dev/null)"; then
    ciphertext="$(jq -r '.content // empty' <<<"$metadata" \
      | tr -d '\n' \
      | base64 -d \
      | tr -d '\r\n')"
    [[ -z "$ciphertext" ]] || break
  fi
  sleep "$CIPHERTEXT_POLL_SECONDS"
done

if [[ -z "$ciphertext" ]]; then
  printf 'encrypted credential was not received for handshake %s\n' "$HANDSHAKE_NONCE" >&2
  exit 1
fi

printf '%s' "$ciphertext" | base64 -d > "$encrypted_token"
openssl pkeyutl \
  -decrypt \
  -inkey "$private_key" \
  -in "$encrypted_token" \
  -pkeyopt rsa_padding_mode:oaep \
  -pkeyopt rsa_oaep_md:sha256 \
  -pkeyopt rsa_mgf1_md:sha256 \
  > "$plain_token"

USER_TOKEN="$(tr -d '\r\n' < "$plain_token")"
if [[ -z "$USER_TOKEN" ]]; then
  echo 'decrypted credential was empty' >&2
  exit 1
fi
printf '::add-mask::%s\n' "$USER_TOKEN"

wait_for_rate_budget
login="$(GH_TOKEN="$USER_TOKEN" gh api user --jq .login)"
if [[ "$login" != ORESoftware ]]; then
  printf 'decrypted credential belongs to unexpected GitHub login: %s\n' "$login" >&2
  exit 1
fi
GH_TOKEN="$USER_TOKEN" gh auth status >/dev/null

{
  printf 'GH_TOKEN=%s\n' "$USER_TOKEN"
  printf 'GITHUB_REPOSITORY_ADMIN_TOKEN=%s\n' "$USER_TOKEN"
} >> "$GITHUB_ENV"

cleanup_credentials
trap - EXIT
printf 'ENCRYPTED_CREDENTIAL_READY login=%s\n' "$login"

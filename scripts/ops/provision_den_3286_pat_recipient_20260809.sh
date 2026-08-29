#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

request_id="${1:?recipient request id required}"
source_commit="${2:?source merge commit required}"
[[ "$request_id" =~ ^den-3286-publish-20260809-[0-9a-f]{16}$ ]]
[[ "$source_commit" =~ ^[0-9a-f]{40}$ ]]

root=/var/lib/oresoftware/ephemeral-pat-recipients
private_key="$root/${request_id}.pem"
metadata="$root/${request_id}.json"
public_key="$(mktemp /tmp/den-3286-pat-recipient-public.XXXXXX.pem)"

cleanup() {
  local status=$?
  rm -f "$public_key"
  exit "$status"
}
trap cleanup EXIT INT TERM

for command in base64 jq openssl sha256sum stat; do
  command -v "$command" >/dev/null
done

install -d -m 700 -o root -g root "$root"
now="$(date +%s)"
expires_at=$((now + 21600))

if [[ -e "$private_key" || -e "$metadata" ]]; then
  [[ -f "$private_key" && ! -L "$private_key" ]]
  [[ -f "$metadata" && ! -L "$metadata" ]]
  [[ "$(stat -c '%u' "$private_key")" == 0 ]]
  mode="$(stat -c '%a' "$private_key")"
  (( (8#$mode & 0077) == 0 ))
  [[ "$(jq -er '.schema_version' "$metadata")" == 1 ]]
  [[ "$(jq -er '.tracking_issue' "$metadata")" == DEN-3286 ]]
  [[ "$(jq -er '.request_id' "$metadata")" == "$request_id" ]]
  [[ "$(jq -er '.source_merge_commit' "$metadata")" == "$source_commit" ]]
  stored_expiry="$(jq -er '.expires_at | floor' "$metadata")"
  (( stored_expiry > now ))
  expires_at="$stored_expiry"
else
  openssl genpkey \
    -algorithm RSA \
    -pkeyopt rsa_keygen_bits:4096 \
    -out "$private_key" >/dev/null 2>&1
  chmod 600 "$private_key"
  chown root:root "$private_key"
  jq -nc \
    --arg request_id "$request_id" \
    --arg source_merge_commit "$source_commit" \
    --argjson created_at "$now" \
    --argjson expires_at "$expires_at" \
    '{
      schema_version: 1,
      tracking_issue: "DEN-3286",
      request_id: $request_id,
      source_merge_commit: $source_merge_commit,
      created_at: $created_at,
      expires_at: $expires_at
    }' > "$metadata"
  chmod 600 "$metadata"
  chown root:root "$metadata"
fi

openssl pkey -in "$private_key" -pubout -out "$public_key"
fingerprint="$(
  openssl pkey -pubin -in "$public_key" -outform DER \
    | sha256sum \
    | awk '{print $1}'
)"
[[ "$fingerprint" =~ ^[0-9a-f]{64}$ ]]
public_key_b64="$(base64 --wrap=0 "$public_key")"

printf 'DEN3286_PAT_RECIPIENT_ID=%s\n' "$request_id"
printf 'DEN3286_PAT_RECIPIENT_FINGERPRINT=%s\n' "$fingerprint"
printf 'DEN3286_PAT_RECIPIENT_EXPIRES_AT=%s\n' "$expires_at"
printf 'DEN3286_PAT_RECIPIENT_PUBLIC_KEY_BASE64=%s\n' "$public_key_b64"
printf 'DEN3286_PAT_RECIPIENT_TOKEN_EXPOSED=false\n'

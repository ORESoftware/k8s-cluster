#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

request_id="${1:?recipient request id required}"
[[ "$request_id" =~ ^ncc-publish-20260805-[0-9a-f]{16}$ ]]
root=/var/lib/oresoftware/ephemeral-pat-recipients
private_key="$root/${request_id}.pem"
metadata="$root/${request_id}.json"
public_key="$(mktemp /tmp/ncc-pat-recipient-public.XXXXXX.pem)"
trap 'rm -f "$public_key"' EXIT

install -d -m 700 -o root -g root "$root"
now="$(date +%s)"
expires_at=$((now + 21600))

if [[ -e "$private_key" || -e "$metadata" ]]; then
  [[ -f "$private_key" && ! -L "$private_key" ]]
  [[ -f "$metadata" && ! -L "$metadata" ]]
  owner="$(stat -c '%u' "$private_key")"
  mode="$(stat -c '%a' "$private_key")"
  [[ "$owner" == 0 ]]
  (( (8#$mode & 0077) == 0 ))
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
    --argjson created_at "$now" \
    --argjson expires_at "$expires_at" \
    '{schema_version:1,request_id:$request_id,created_at:$created_at,expires_at:$expires_at}' \
    > "$metadata"
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

printf 'NCC_PAT_RECIPIENT_ID=%s\n' "$request_id"
printf 'NCC_PAT_RECIPIENT_FINGERPRINT=%s\n' "$fingerprint"
printf 'NCC_PAT_RECIPIENT_EXPIRES_AT=%s\n' "$expires_at"
printf 'NCC_PAT_RECIPIENT_PUBLIC_KEY_BASE64=%s\n' "$public_key_b64"

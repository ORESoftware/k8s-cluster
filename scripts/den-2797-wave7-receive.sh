#!/usr/bin/env bash
set -Eeuo pipefail
umask 077
control_api="https://api.github.com/repos/${CONTROL_REPOSITORY}"

control_call() {
  local method="$1" url="$2" data="${3:-}"
  local args=(
    --fail-with-body --silent --show-error
    --request "$method"
    --url "$url"
    --header 'Accept: application/vnd.github+json'
    --header "Authorization: Bearer ${CONTROL_TOKEN}"
    --header 'X-GitHub-Api-Version: 2022-11-28'
  )
  if [[ -n "$data" ]]; then
    args+=(--data-binary "$data")
  fi
  curl "${args[@]}"
}

delete_control_file() {
  local path="$1" message="$2"
  local metadata sha payload
  if metadata="$(control_call GET "${control_api}/contents/${path}?ref=${CONTROL_BRANCH}" 2>/dev/null)"; then
    sha="$(jq -r '.sha // empty' <<<"$metadata")"
    if [[ -n "$sha" ]]; then
      payload="$(jq -nc --arg message "$message" --arg sha "$sha" --arg branch "$CONTROL_BRANCH" \
        '{message:$message,sha:$sha,branch:$branch}')"
      control_call DELETE "${control_api}/contents/${path}" "$payload" >/dev/null || true
    fi
  fi
}

put_control_file() {
  local path="$1" source="$2" message="$3"
  local encoded payload
  encoded="$(base64 -w0 "$source")"
  payload="$(jq -nc --arg message "$message" --arg content "$encoded" --arg branch "$CONTROL_BRANCH" \
    '{message:$message,content:$content,branch:$branch}')"
  control_call PUT "${control_api}/contents/${path}" "$payload" >/dev/null
}

cleanup() {
  delete_control_file "$PUBLIC_KEY_PATH" 'ops(DEN-2797): remove expired Wave 7 public key'
  delete_control_file "$CIPHERTEXT_PATH" 'ops(DEN-2797): remove encrypted Wave 7 credential'
  shred -u \
    /tmp/den-2797-wave7-private.pem \
    /tmp/den-2797-wave7-public.pem \
    /tmp/den-2797-wave7-token.enc \
    /tmp/den-2797-wave7-token.txt \
    2>/dev/null || true
}
trap cleanup EXIT

delete_control_file "$PUBLIC_KEY_PATH" 'ops(DEN-2797): remove stale Wave 7 public key'
delete_control_file "$CIPHERTEXT_PATH" 'ops(DEN-2797): remove stale encrypted Wave 7 credential'

openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:4096 \
  -out /tmp/den-2797-wave7-private.pem >/dev/null 2>&1
openssl pkey -in /tmp/den-2797-wave7-private.pem -pubout \
  -out /tmp/den-2797-wave7-public.pem >/dev/null 2>&1
put_control_file "$PUBLIC_KEY_PATH" /tmp/den-2797-wave7-public.pem \
  'ops(DEN-2797): publish ephemeral Wave 7 public key'
echo "HANDSHAKE_READY nonce=${HANDSHAKE_NONCE} path=${PUBLIC_KEY_PATH}"

ciphertext=''
for _ in $(seq 1 720); do
  if metadata="$(control_call GET "${control_api}/contents/${CIPHERTEXT_PATH}?ref=${CONTROL_BRANCH}" 2>/dev/null)"; then
    ciphertext="$(jq -r '.content // empty' <<<"$metadata" | tr -d '\n' | base64 -d | tr -d '\r\n')"
    [[ -z "$ciphertext" ]] || break
  fi
  sleep 5
done
if [[ -z "$ciphertext" ]]; then
  echo "encrypted credential was not received for handshake ${HANDSHAKE_NONCE}" >&2
  exit 1
fi

printf '%s' "$ciphertext" | base64 -d > /tmp/den-2797-wave7-token.enc
openssl pkeyutl -decrypt \
  -inkey /tmp/den-2797-wave7-private.pem \
  -in /tmp/den-2797-wave7-token.enc \
  -pkeyopt rsa_padding_mode:oaep \
  -pkeyopt rsa_oaep_md:sha256 \
  -pkeyopt rsa_mgf1_md:sha256 \
  > /tmp/den-2797-wave7-token.txt

OWNER_TOKEN="$(tr -d '\r\n' < /tmp/den-2797-wave7-token.txt)"
test "${#OWNER_TOKEN}" -ge 20
echo "::add-mask::$OWNER_TOKEN"
printf 'GH_TOKEN=%s\n' "$OWNER_TOKEN" >> "$GITHUB_ENV"
cleanup
trap - EXIT

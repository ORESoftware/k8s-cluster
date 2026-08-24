#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

TARGET_USERNAME="${1:?target username required}"
PR_NUMBER="${2:?pull request number required}"
RUN_ID="${3:?workflow run id required}"
REPOSITORY="${4:?repository required}"
WORKSPACE="${5:?workspace required}"
ACTIONS_TOKEN="${ACTIONS_TOKEN:?ACTIONS_TOKEN required}"

[[ "$TARGET_USERNAME" =~ ^[A-Za-z0-9]([A-Za-z0-9-]{0,37}[A-Za-z0-9])?$ ]]
[[ "$TARGET_USERNAME" != *--* ]]
[[ "$PR_NUMBER" =~ ^[0-9]+$ ]]
[[ "$RUN_ID" =~ ^[0-9]+$ ]]
[[ "$REPOSITORY" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]]
[[ "$WORKSPACE" == /* ]]

dir="$(mktemp -d "${RUNNER_TEMP:-/tmp}/org-member-ephemeral.XXXXXX")"
private_key="$dir/private.pem"
public_key="$dir/public.pem"
ciphertext_file="$dir/token.enc"
plaintext_file="$dir/token.bin"
json_report="$dir/report.json"
markdown_report="$dir/report.md"
comment_body="$dir/comment.md"
stage="initialization"
completed=false

cleanup() {
  unset ACTIONS_TOKEN GH_TOKEN GITHUB_TOKEN org_token public_key_b64 envelope_body envelope_line ciphertext fingerprint
  rm -rf "$dir"
}

post_comment() {
  local body_file="${1:?comment body file required}"
  local payload="$dir/comment-payload.json"
  jq -n --rawfile body "$body_file" '{body:$body}' > "$payload"
  GH_TOKEN="$ACTIONS_TOKEN" gh api \
    --method POST \
    "repos/$REPOSITORY/issues/$PR_NUMBER/comments" \
    --input "$payload" \
    >/dev/null
}

report_failure() {
  local status=$?
  trap - EXIT ERR
  set +e
  {
    echo '<!-- org-member-ephemeral-result-v1 -->'
    echo '### Ephemeral organization membership reconciliation failed'
    echo
    echo "- Target: \`$TARGET_USERNAME\`"
    echo "- Run ID: \`$RUN_ID\`"
    echo "- Last bounded stage: \`$stage\`"
    echo "- Exit code: \`$status\`"
    echo
    echo 'No PAT, decrypted credential, private key, or raw GitHub response is included.'
  } > "$comment_body"
  post_comment "$comment_body" || true
  cleanup
  exit "$status"
}
trap report_failure ERR EXIT

for command in openssl base64 sha256sum jq gh python3; do
  command -v "$command" >/dev/null 2>&1
 done

test -f "$WORKSPACE/scripts/ops/invite_org_member_all.py"
python3 -m py_compile "$WORKSPACE/scripts/ops/invite_org_member_all.py"

stage="key-generation"
openssl genpkey \
  -algorithm RSA \
  -pkeyopt rsa_keygen_bits:4096 \
  -out "$private_key" \
  >/dev/null 2>&1
chmod 0600 "$private_key"
openssl pkey -in "$private_key" -pubout -out "$public_key" >/dev/null 2>&1
chmod 0600 "$public_key"

fingerprint="$(
  openssl pkey -pubin -in "$public_key" -outform DER 2>/dev/null \
    | sha256sum \
    | awk '{print $1}'
)"
[[ "$fingerprint" =~ ^[0-9a-f]{64}$ ]]
public_key_b64="$(base64 --wrap=0 "$public_key")"
[[ "$public_key_b64" =~ ^[A-Za-z0-9+/=]+$ ]]

stage="public-key-publication"
{
  echo "<!-- org-member-ephemeral-key-v1 run=$RUN_ID fingerprint=$fingerprint -->"
  echo '### Ephemeral organization membership key'
  echo
  echo "- Target: \`$TARGET_USERNAME\`"
  echo "- Run ID: \`$RUN_ID\`"
  echo "- Fingerprint: \`$fingerprint\`"
  echo '- Contract: RSA-4096 OAEP-SHA256 with MGF1-SHA256'
  echo '- Lifetime: this running job only'
  echo
  echo 'Public key (base64-encoded PEM; safe to disclose):'
  echo
  echo '```text'
  echo "$public_key_b64"
  echo '```'
} > "$comment_body"
post_comment "$comment_body"
unset public_key_b64

stage="ciphertext-wait"
marker="<!-- org-member-ephemeral-ciphertext-v1 run=$RUN_ID fingerprint=$fingerprint -->"
envelope_body=""
for attempt in $(seq 1 150); do
  envelope_body="$(
    GH_TOKEN="$ACTIONS_TOKEN" gh api \
      "repos/$REPOSITORY/issues/$PR_NUMBER/comments?per_page=100" \
      --jq ".[] | select(.body | contains(\"$marker\")) | .body" \
      | tail -n 1
  )"
  if [[ -n "$envelope_body" ]]; then
    break
  fi
  if (( attempt % 12 == 0 )); then
    printf 'ORG_MEMBER_EPHEMERAL_STAGE stage=ciphertext-wait elapsed_seconds=%d\n' "$((attempt * 4))"
  fi
  sleep 4
 done
[[ -n "$envelope_body" ]]

envelope_line="$(
  printf '%s\n' "$envelope_body" \
    | grep -E '^ORG_MEMBER_TOKEN_ENVELOPE_V1 fingerprint=[0-9a-f]{64} ciphertext=[A-Za-z0-9+/=]+$' \
    | tail -n 1
)"
unset envelope_body marker
[[ "$envelope_line" =~ ^ORG_MEMBER_TOKEN_ENVELOPE_V1[[:space:]]fingerprint=([0-9a-f]{64})[[:space:]]ciphertext=([A-Za-z0-9+/=]+)$ ]]
test "${BASH_REMATCH[1]}" = "$fingerprint"
ciphertext="${BASH_REMATCH[2]}"
unset envelope_line
printf '%s' "$ciphertext" | base64 --decode > "$ciphertext_file"
unset ciphertext
test "$(stat -c '%s' "$ciphertext_file")" -eq 512

stage="credential-decryption"
openssl pkeyutl \
  -decrypt \
  -inkey "$private_key" \
  -pkeyopt rsa_padding_mode:oaep \
  -pkeyopt rsa_oaep_md:sha256 \
  -pkeyopt rsa_mgf1_md:sha256 \
  -in "$ciphertext_file" \
  -out "$plaintext_file" \
  >/dev/null 2>&1
rm -f "$ciphertext_file" "$private_key" "$public_key"
org_token="$(cat "$plaintext_file")"
rm -f "$plaintext_file"
test -n "$org_token"
[[ "$org_token" != *$'\n'* ]]
[[ "$org_token" != *$'\r'* ]]
[[ "$org_token" != *$'\t'* ]]
[[ "$org_token" != *' '* ]]
printf '::add-mask::%s\n' "$org_token"

stage="organization-reconciliation"
set +e
GH_TOKEN="$org_token" python3 "$WORKSPACE/scripts/ops/invite_org_member_all.py" \
  --execute \
  --username "$TARGET_USERNAME" \
  --expected-authenticated-login ORESoftware \
  --json-report "$json_report" \
  --markdown-report "$markdown_report"
inviter_status=$?
set -e
unset org_token GH_TOKEN GITHUB_TOKEN

if [[ "$inviter_status" -ne 0 ]]; then
  if [[ -s "$markdown_report" ]]; then
    {
      echo '<!-- org-member-ephemeral-result-v1 -->'
      cat "$markdown_report"
      echo
      echo "Ephemeral broker run ID: \`$RUN_ID\`"
    } > "$comment_body"
    post_comment "$comment_body" || true
  fi
  exit "$inviter_status"
fi

grep -Fq '<!-- org-member-invitation-report-complete -->' "$markdown_report"
test "$(jq -r '.mode' "$json_report")" = execute
test "$(jq -r '.authenticated_login | ascii_downcase' "$json_report")" = oresoftware
test "$(jq -r '.target_username | ascii_downcase' "$json_report")" = "${TARGET_USERNAME,,}"
test "$(jq -r '.counts.failed // 0' "$json_report")" -eq 0

stage="result-publication"
{
  echo '<!-- org-member-ephemeral-result-v1 -->'
  cat "$markdown_report"
  echo
  echo "Ephemeral broker run ID: \`$RUN_ID\`"
  echo "Credential transport: \`RSA-4096-OAEP-SHA256, one-run key\`"
} > "$comment_body"
post_comment "$comment_body"
cat "$markdown_report"

completed=true
trap - ERR EXIT
cleanup

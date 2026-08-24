#!/usr/bin/env bash
set -euo pipefail
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
comments_file="$dir/comments.json"
envelope_file="$dir/envelope.txt"
envelope_parts="$dir/envelope.json"
json_report="$dir/report.json"
markdown_report="$dir/report.md"
comment_body="$dir/comment.md"
stage="initialization"

cleanup() {
  unset ACTIONS_TOKEN GH_TOKEN GITHUB_TOKEN org_token public_key_b64 ciphertext fingerprint
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
  trap - ERR
  set +e
  printf 'ORG_MEMBER_EPHEMERAL_ERROR stage=%s code=%s\n' "$stage" "$status" >&2
  if [[ -d "$dir" ]]; then
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
  fi
  cleanup
  exit "$status"
}
trap report_failure ERR

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

stage="ciphertext-poll"
marker="<!-- org-member-ephemeral-ciphertext-v1 run=$RUN_ID fingerprint=$fingerprint -->"
: > "$envelope_file"
for attempt in $(seq 1 150); do
  tmp_comments="$comments_file.tmp"
  set +e
  GH_TOKEN="$ACTIONS_TOKEN" gh api \
    "repos/$REPOSITORY/issues/$PR_NUMBER/comments?per_page=100" \
    > "$tmp_comments"
  api_status=$?
  set -e

  if [[ "$api_status" -eq 0 ]] && jq -e 'type == "array"' "$tmp_comments" >/dev/null 2>&1; then
    mv "$tmp_comments" "$comments_file"
    jq -r --arg marker "$marker" \
      '[.[] | select(((.body // "") | contains($marker))) | .body] | last // ""' \
      "$comments_file" > "$envelope_file"
    if [[ -s "$envelope_file" ]]; then
      break
    fi
  else
    rm -f "$tmp_comments"
    printf 'ORG_MEMBER_EPHEMERAL_STAGE stage=ciphertext-poll attempt=%d api_status=%d action=retry\n' \
      "$attempt" "$api_status"
  fi

  if (( attempt % 12 == 0 )); then
    printf 'ORG_MEMBER_EPHEMERAL_STAGE stage=ciphertext-poll elapsed_seconds=%d\n' "$((attempt * 4))"
  fi
  sleep 4
done
[[ -s "$envelope_file" ]]

stage="ciphertext-parse"
python3 - "$envelope_file" "$fingerprint" > "$envelope_parts" <<'PY'
import json
from pathlib import Path
import re
import sys

body = Path(sys.argv[1]).read_text(encoding="utf-8", errors="strict")
expected_fingerprint = sys.argv[2]
pattern = re.compile(
    r"^ORG_MEMBER_TOKEN_ENVELOPE_V1 "
    r"fingerprint=([0-9a-f]{64}) "
    r"ciphertext=([A-Za-z0-9+/=]+)$"
)
matches = [pattern.fullmatch(line) for line in body.splitlines()]
matches = [match for match in matches if match is not None]
if len(matches) != 1:
    raise SystemExit(65)
match = matches[0]
if match.group(1) != expected_fingerprint:
    raise SystemExit(66)
json.dump({"ciphertext": match.group(2)}, sys.stdout, separators=(",", ":"))
PY
ciphertext="$(jq -er '.ciphertext' "$envelope_parts")"
[[ "$ciphertext" =~ ^[A-Za-z0-9+/=]+$ ]]

stage="ciphertext-decode"
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

trap - ERR
cleanup

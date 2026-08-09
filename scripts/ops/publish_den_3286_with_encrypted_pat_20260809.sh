#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

trusted_root="${1:?trusted checkout root required}"
request_path="${2:?encrypted request path required}"
[[ -d "$trusted_root/.git" ]]
[[ "$request_path" == ops/requests/den-3286-encrypted-pat-20260809.json ]]
request_file="$trusted_root/$request_path"
[[ -f "$request_file" && ! -L "$request_file" ]]

work="$(mktemp -d /tmp/den-3286-encrypted-pat-publisher.XXXXXX)"
request_id=''
private_key=''
metadata=''
plain_token="$work/token"
ciphertext="$work/token.enc"
manifest_encoded="$work/manifest.gz.b64"
publisher_encoded="$work/publisher.gz.b64"
manifest="$work/test-org-expansion-20260808.json"
publisher="$work/publish_test_org_expansion_20260808.py"
report="$work/den-3286-test-org-expansion-publication.json"
markdown="$work/den-3286-test-org-expansion-publication.md"
publisher_stdout="$work/publisher.stdout"
publisher_stderr="$work/publisher.stderr"

cleanup() {
  local status=$?
  unset PROTECTED_GITHUB_TOKEN GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN token_value
  if [[ -n "$private_key" ]]; then
    rm -f "$private_key" "${metadata:-}"
  fi
  find "$work" -type f -exec sh -c 'for file do : > "$file"; done' sh {} + 2>/dev/null || true
  rm -rf "$work"
  exit "$status"
}
trap cleanup EXIT INT TERM

for command in base64 git gzip jq openssl python3 sha256sum stat tar; do
  command -v "$command" >/dev/null
done

jq -e '
  .schema_version == 1 and
  .execute == true and
  .tracking_issue == "DEN-3286" and
  .recipient_id == "den-3286-publish-20260809-2b9b8273dfbbc871" and
  (.recipient_fingerprint | type == "string" and test("^[0-9a-f]{64}$")) and
  .manifest_sha256 == "5d1cf8cb7af82a81660bf2fe7536759c7b15bd01bef4a4a04730095ad998d056" and
  .publisher_sha256 == "7578de348dab3221e70ed9b02ab22c99c624fe3c02dff8b0fd89a77ec34f71bb" and
  .runtime_patch_sha256 == "c4fd9a6ac616e83a48228b6a689f4a28428c7a6242c03e261a95b6cee9c01f03" and
  .expected_organization_count == 25 and
  .repositories_per_organization == 4 and
  .expected_repository_count == 100 and
  (.ciphertext_base64 | type == "string" and test("^[A-Za-z0-9+/]+={0,2}$") and length >= 600) and
  ((tostring | test("gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|lin_api_[A-Za-z0-9]{20,}")) | not)
' "$request_file" >/dev/null

request_id="$(jq -er '.recipient_id' "$request_file")"
expected_fingerprint="$(jq -er '.recipient_fingerprint' "$request_file")"
root=/var/lib/oresoftware/ephemeral-pat-recipients
private_key="$root/${request_id}.pem"
metadata="$root/${request_id}.json"
[[ -f "$private_key" && ! -L "$private_key" ]]
[[ -f "$metadata" && ! -L "$metadata" ]]
[[ "$(stat -c '%u' "$private_key")" == 0 ]]
mode="$(stat -c '%a' "$private_key")"
(( (8#$mode & 0077) == 0 ))
now="$(date +%s)"
expires_at="$(jq -er '.expires_at | floor' "$metadata")"
(( expires_at > now ))
[[ "$(jq -er '.schema_version' "$metadata")" == 1 ]]
[[ "$(jq -er '.tracking_issue' "$metadata")" == DEN-3286 ]]
[[ "$(jq -er '.request_id' "$metadata")" == "$request_id" ]]

observed_fingerprint="$(
  openssl pkey -in "$private_key" -pubout 2>/dev/null \
    | openssl pkey -pubin -outform DER 2>/dev/null \
    | sha256sum \
    | awk '{print $1}'
)"
[[ "$observed_fingerprint" == "$expected_fingerprint" ]]

jq -er '.ciphertext_base64' "$request_file" | base64 --decode > "$ciphertext"
openssl pkeyutl -decrypt \
  -inkey "$private_key" \
  -pkeyopt rsa_padding_mode:oaep \
  -pkeyopt rsa_oaep_md:sha256 \
  -in "$ciphertext" \
  -out "$plain_token"
chmod 600 "$plain_token"
token_value="$(cat "$plain_token")"
: > "$plain_token"

if [[ ! "$token_value" =~ ^(gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})$ ]]; then
  echo 'decrypted credential has an unexpected format' >&2
  exit 65
fi
export PROTECTED_GITHUB_TOKEN="$token_value"
export GH_TOKEN="$token_value"
export GITHUB_REPOSITORY_ADMIN_TOKEN="$token_value"
unset token_value

manifest_carrier="$trusted_root/scripts/ops/test-org-expansion-20260808.json.gz.b64"
publisher_carrier="$trusted_root/scripts/ops/publish_test_org_expansion_20260808.py.gz.b64"
test -s "$manifest_carrier"
test -s "$publisher_carrier"
tr -d '\r\n' < "$manifest_carrier" > "$manifest_encoded"
tr -d '\r\n' < "$publisher_carrier" > "$publisher_encoded"
test "$(wc -c < "$manifest_encoded" | tr -d '[:space:]')" = 7400
test "$(wc -c < "$publisher_encoded" | tr -d '[:space:]')" = 15908
printf '%s  %s\n' \
  ee5e11b7683e12eed137a22a2656ae42bc230e3167e4b5280aa3445bec11e2ce \
  "$manifest_encoded" | sha256sum --check --strict
printf '%s  %s\n' \
  9fda9fd05bc1ffd7ef65ff85391fd848d7d77b779a56b9517d2c6792822d552a \
  "$publisher_encoded" | sha256sum --check --strict
base64 --decode < "$manifest_encoded" | gzip --decompress > "$manifest"
base64 --decode < "$publisher_encoded" | gzip --decompress > "$publisher"
chmod 700 "$publisher"
printf '%s  %s\n' \
  '5d1cf8cb7af82a81660bf2fe7536759c7b15bd01bef4a4a04730095ad998d056' \
  "$manifest" | sha256sum --check --strict
printf '%s  %s\n' \
  '7578de348dab3221e70ed9b02ab22c99c624fe3c02dff8b0fd89a77ec34f71bb' \
  "$publisher" | sha256sum --check --strict

python3 - "$publisher" <<'PY'
import ast
import sys
from pathlib import Path

path = Path(sys.argv[1])
text = path.read_text(encoding="utf-8")
replacements = {
    'target.write_text(content, encoding="utf-8", newline="\\n")':
        'target.write_text(content, encoding="utf-8")',
    '            encoding="utf-8",\n            newline="\\n",\n':
        '            encoding="utf-8",\n',
    'args.markdown.write_text(markdown, encoding="utf-8", newline="\\n")':
        'args.markdown.write_text(markdown, encoding="utf-8")',
}
for old, new in replacements.items():
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"host compatibility patch anchor count={count}: {old!r}")
    text = text.replace(old, new, 1)
path.write_bytes(text.encode("utf-8"))
tree = ast.parse(text, filename=str(path))
for node in ast.walk(tree):
    if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute):
        if node.func.attr == "write_text" and any(
            keyword.arg == "newline" for keyword in node.keywords
        ):
            raise SystemExit(
                f"version-coupled Path.write_text(newline=) remains at line {node.lineno}"
            )
PY
printf '%s  %s\n' \
  'c4fd9a6ac616e83a48228b6a689f4a28428c7a6242c03e261a95b6cee9c01f03' \
  "$publisher" | sha256sum --check --strict
python3 -m py_compile "$publisher"

export RUNNER_TEMP="$work"
export TRUSTED_SHA="$(git -C "$trusted_root" rev-parse HEAD)"
[[ "$TRUSTED_SHA" =~ ^[0-9a-f]{40}$ ]]

set +e
python3 "$publisher" \
  --manifest "$manifest" \
  --report "$report" \
  --markdown "$markdown" >"$publisher_stdout" 2>"$publisher_stderr"
status=$?
set -e
if (( status != 0 )); then
  python3 - "$publisher_stderr" "$PROTECTED_GITHUB_TOKEN" <<'PY' >&2
import sys
from pathlib import Path
path = Path(sys.argv[1])
token = sys.argv[2]
text = path.read_text(encoding="utf-8", errors="replace").replace(token, "[REDACTED]")
print(text[-16000:])
PY
  exit "$status"
fi

jq -e \
  --arg trusted "$TRUSTED_SHA" '
    .schema_version == 1 and
    .tracking_issue == "DEN-3286" and
    .source_commit == $trusted and
    .publisher_identity == "ORESoftware" and
    .credential_type == "protected-host-pat" and
    .token_exposed == false and
    .force_push_used == false and
    .verified_organization_count == 25 and
    .verified_repository_count == 100
  ' "$report" >/dev/null

if grep -Eq 'gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|lin_api_[A-Za-z0-9]{20,}' \
  "$report" "$markdown" "$publisher_stdout"; then
  echo 'credential-shaped content detected in publication evidence' >&2
  exit 1
fi

printf 'DEN3286_ENCRYPTED_PAT_BUNDLE_GZIP_BASE64='
tar -C "$work" -cf - \
  den-3286-test-org-expansion-publication.json \
  den-3286-test-org-expansion-publication.md \
  | gzip -9 \
  | base64 --wrap=0
printf '\n'
printf 'DEN3286_ENCRYPTED_PAT_PUBLICATION_COMPLETE recipient=%s token_exposed=false\n' "$request_id"

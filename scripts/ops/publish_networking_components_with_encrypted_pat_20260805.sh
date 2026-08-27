#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

trusted_root="${1:?trusted checkout root required}"
request_path="${2:?encrypted request path required}"
[[ -d "$trusted_root/.git" ]]
[[ "$request_path" == ops/requests/networking-components-encrypted-pat-20260805.json ]]
request_file="$trusted_root/$request_path"
[[ -f "$request_file" && ! -L "$request_file" ]]

work="$(mktemp -d /tmp/ncc-encrypted-pat-publisher.XXXXXX)"
request_id=''
private_key=''
metadata=''
plain_token="$work/token"
ciphertext="$work/token.enc"
publisher_stdout="$work/publisher.stdout"
publisher_stderr="$work/publisher.stderr"

cleanup() {
  local status=$?
  unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN token_value
  if [[ -n "$private_key" ]]; then
    rm -f "$private_key" "${metadata:-}"
  fi
  find "$work" -type f -exec sh -c 'for file do : > "$file"; done' sh {} + 2>/dev/null || true
  rm -rf "$work"
  exit "$status"
}
trap cleanup EXIT INT TERM

for command in base64 jq openssl python3 sha256sum stat; do
  command -v "$command" >/dev/null
done

jq -e '
  .schema_version == 1 and
  .execute == true and
  .organization == "networking-components" and
  .visibility == "private" and
  .default_branch == "dev" and
  .repository_count == 10 and
  (.recipient_id | type == "string" and test("^ncc-publish-20260805-[0-9a-f]{16}$")) and
  (.recipient_fingerprint | type == "string" and test("^[0-9a-f]{64}$")) and
  (.ciphertext_base64 | type == "string" and test("^[A-Za-z0-9+/]+={0,2}$")) and
  (.repositories == [
    "ncc-dhcp-server",
    "ncc-ipam",
    "ncc-firewall",
    "ncc-forward-proxy",
    "ncc-ntp",
    "ncc-stun-turn",
    "ncc-service-discovery",
    "ncc-network-controller",
    "ncc-observability",
    "ncc-pki"
  ])
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
export GH_TOKEN="$token_value"
export GITHUB_REPOSITORY_ADMIN_TOKEN="$token_value"
unset token_value

publisher="$trusted_root/scripts/ops/publish_networking_components_20260805.py"
python3 -m py_compile "$publisher"
set +e
python3 "$publisher" --request "$request_file" >"$publisher_stdout" 2>"$publisher_stderr"
status=$?
set -e
if (( status != 0 )); then
  python3 - "$publisher_stderr" "$GH_TOKEN" <<'PY' >&2
import sys
from pathlib import Path
path = Path(sys.argv[1])
token = sys.argv[2]
text = path.read_text(encoding="utf-8", errors="replace").replace(token, "[REDACTED]")
print(text[-12000:])
PY
  exit "$status"
fi
cat "$publisher_stdout"
printf 'NCC_ENCRYPTED_PAT_PUBLICATION_COMPLETE recipient=%s token_exposed=false\n' "$request_id"

#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

trusted_root="${1:?trusted checkout root required}"
request_path="${2:?request path required}"
request_file="$trusted_root/$request_path"
[[ -d "$trusted_root/.git" && -f "$request_file" && ! -L "$request_file" ]]
[[ "$request_path" == ops/requests/memebank-create-recovery-repos-20260807.json ]]
for c in base64 git jq openssl sha256sum stat; do command -v "$c" >/dev/null; done

work="$(mktemp -d /tmp/memebank-create-recovery-repos.XXXXXX)"
cleanup() {
  rc=$?
  unset GH_TOKEN GITHUB_TOKEN token
  find "$work" -type f -exec sh -c 'for f do : > "$f"; done' sh {} + 2>/dev/null || true
  rm -rf "$work"
  exit "$rc"
}
trap cleanup EXIT INT TERM

jq -e '
 .schema_version == 1 and .execute == true and
 .organization == "memebank" and .visibility == "private" and
 .recipient_id == "ncc-publish-20260805-f1f811bc7a369f44" and
 .recipient_fingerprint == "85df429fe9dbb795ebe8cab6c428883e2028f94a9e0017fcc00e70bab08daaf7" and
 .recipient_expires_at == 1786093975 and
 (.token_ciphertext_sha256 | test("^[0-9a-f]{64}$")) and
 (.token_ciphertext_base64 | test("^[A-Za-z0-9+/]+={0,2}$")) and
 (.targets | length) == 13 and
 all(.targets[];
   (.source | type == "string" and length > 0) and
   (.conflict_repository | test("^memebank/[A-Za-z0-9._-]+$")) and
   (.target | test("^memebank/[A-Za-z0-9._-]+-2$")) and
   (.source_root | test("^[0-9a-f]{40}$")) and
   (.source_tip | test("^[0-9a-f]{40}$"))
 )
' "$request_file" >/dev/null

jq -r '.targets[].target' "$request_file" | sort -u > "$work/targets.txt"
test "$(wc -l < "$work/targets.txt" | tr -d ' ')" = 13
cat > "$work/expected.txt" <<'EOF'
memebank/.github-2
memebank/mb-cli-2
memebank/mb-clients-2
memebank/mb-infra-2
memebank/mb-interfaces-2
memebank/memebank-api-server.rs-2
memebank/memebank-e2e-2
memebank/memebank-flutter-2
memebank/memebank-mcp-server.rs-2
memebank/memebank-media-worker.rs-2
memebank/memebank-monorepo-2
memebank/memebank-web-server.rs-2
memebank/memebank.github.io-2
EOF
sort -o "$work/expected.txt" "$work/expected.txt"
cmp -s "$work/expected.txt" "$work/targets.txt"

recipient_id="$(jq -er '.recipient_id' "$request_file")"
private_key="/var/lib/oresoftware/ephemeral-pat-recipients/${recipient_id}.pem"
metadata="/var/lib/oresoftware/ephemeral-pat-recipients/${recipient_id}.json"
[[ -f "$private_key" && ! -L "$private_key" && -f "$metadata" && ! -L "$metadata" ]]
[[ "$(stat -c '%u' "$private_key")" == 0 ]]
mode="$(stat -c '%a' "$private_key")"; (( (8#$mode & 0077) == 0 ))
now="$(date +%s)"; expires="$(jq -er '.expires_at | floor' "$metadata")"
(( expires > now ))
test "$expires" = "$(jq -er '.recipient_expires_at' "$request_file")"
test "$(jq -er '.request_id' "$metadata")" = "$recipient_id"
fingerprint="$(openssl pkey -in "$private_key" -pubout 2>/dev/null | openssl pkey -pubin -outform DER 2>/dev/null | sha256sum | awk '{print $1}')"
test "$fingerprint" = "$(jq -er '.recipient_fingerprint' "$request_file")"

cipher="$work/token.enc"
token_file="$work/token"
jq -er '.token_ciphertext_base64' "$request_file" | base64 -d > "$cipher"
printf '%s  %s\n' "$(jq -er '.token_ciphertext_sha256' "$request_file")" "$cipher" | sha256sum --check --strict
openssl pkeyutl -decrypt -inkey "$private_key" \
  -pkeyopt rsa_padding_mode:oaep -pkeyopt rsa_oaep_md:sha256 \
  -in "$cipher" -out "$token_file"
token="$(cat "$token_file")"; : > "$token_file"
[[ "$token" =~ ^(gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})$ ]]
export GH_TOKEN="$token" GITHUB_TOKEN="$token"; unset token

if ! command -v gh >/dev/null; then
  archive="$work/gh.tar.gz"
  python3 - "$archive" <<'PY'
import sys, urllib.request
u='https://github.com/cli/cli/releases/download/v2.97.0/gh_2.97.0_linux_amd64.tar.gz'
r=urllib.request.Request(u,headers={'User-Agent':'memebank-recovery-repo-creator/1'})
with urllib.request.urlopen(r,timeout=180) as x: open(sys.argv[1],'wb').write(x.read())
PY
  printf '%s  %s\n' 'a2c9b8497e1f85b1ad0dfcb78b5a622e098801b8e461e459e88e1ee12f018112' "$archive" | sha256sum --check --strict
  tar -xzf "$archive" -C "$work"
  export PATH="$work/gh_2.97.0_linux_amd64/bin:$PATH"
fi

gh auth status >/dev/null
test "$(gh api user --jq .login)" = ORESoftware
membership="$(gh api user/memberships/orgs/memebank)"
test "$(jq -er .state <<<"$membership")" = active
test "$(jq -er .role <<<"$membership")" = admin

# Re-prove no shared root or tip before creating any recovery namespace.
while IFS= read -r item; do
  conflict="$(jq -er .conflict_repository <<<"$item")"
  root="$(jq -er .source_root <<<"$item")"
  tip="$(jq -er .source_tip <<<"$item")"
  gh api "repos/$conflict" >/dev/null
  if gh api "repos/$conflict/git/commits/$root" >/dev/null 2>&1; then
    echo "Refusing: source root unexpectedly exists in $conflict" >&2; exit 1
  fi
  if gh api "repos/$conflict/git/commits/$tip" >/dev/null 2>&1; then
    echo "Refusing: source tip unexpectedly exists in $conflict" >&2; exit 1
  fi
done < <(jq -c '.targets[]' "$request_file")

ledger="$work/ledger.jsonl"; : > "$ledger"
while IFS= read -r item; do
  source="$(jq -er .source <<<"$item")"
  target="$(jq -er .target <<<"$item")"
  name="${target#memebank/}"
  created=false
  if gh api "repos/$target" > "$work/repo.json" 2>/dev/null; then
    test "$(jq -er .private "$work/repo.json")" = true
    test "$(jq -er .size "$work/repo.json")" = 0
    refs="$(gh api "repos/$target/git/matching-refs/heads/" 2>/dev/null || printf '[]')"
    test "$(jq 'length' <<<"$refs")" = 0
  else
    gh api --method POST orgs/memebank/repos \
      -f name="$name" \
      -f description="Preserved Memebank recovery history for $source; isolated as -2 because the existing namespace has unrelated Git history." \
      -F private=true -F auto_init=false -F has_issues=true -F has_projects=true -F has_wiki=false \
      > "$work/repo.json"
    created=true
  fi
  test "$(jq -er .full_name "$work/repo.json")" = "$target"
  test "$(jq -er .private "$work/repo.json")" = true
  id="$(jq -er .id "$work/repo.json")"
  url="$(jq -er .html_url "$work/repo.json")"
  jq -nc --arg source "$source" --arg target "$target" --arg url "$url" --argjson id "$id" --argjson created "$created" \
    '{source:$source,target:$target,id:$id,url:$url,created:$created}' >> "$ledger"
  printf 'MEMEBANK_RECOVERY_REPO_READY=%s|%s|%s\n' "$target" "$id" "$created"
done < <(jq -c '.targets[]' "$request_file")

test "$(wc -l < "$ledger" | tr -d ' ')" = 13
printf 'MEMEBANK_RECOVERY_REPOS_COMPLETE=13\n'
printf 'MEMEBANK_RECOVERY_REPO_LEDGER_SHA256=%s\n' "$(sha256sum "$ledger" | awk '{print $1}')"

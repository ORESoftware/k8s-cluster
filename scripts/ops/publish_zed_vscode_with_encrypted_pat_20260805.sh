#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

trusted_root="${1:?trusted checkout root required}"
request_path="${2:?encrypted request path required}"
request_file="$trusted_root/$request_path"
[[ -d "$trusted_root/.git" ]]
[[ "$request_path" == ops/requests/zed-vscode-encrypted-pat-20260805.json ]]
[[ -f "$request_file" && ! -L "$request_file" ]]

for command in base64 git gh jq openssl sha256sum stat; do
  command -v "$command" >/dev/null
done

jq -e '
  .schema_version == 1 and .execute == true and
  .organization == "zed-pkg" and .repository == "zed-pkg/zed-vscode" and
  .visibility == "public" and .default_branch == "main" and
  .recipient_id == "ncc-publish-20260805-81f87008997e0af7" and
  .recipient_fingerprint == "f2f5515ee2f22718318b5d88e27226ae3db9e5231954715f5dd16d6f59dec831" and
  .source_repository == "zed-pkg/.github" and
  .source_commit == "62b920b818e1255cd8643b47fc614133a7287a38" and
  .bundle_sha256 == "a0d927aa62ab20922e85ee2de9592d0a9fd4bafebf6a5b00a0029a95914c2991" and
  .initial_sha == "376372168e12ddd0d2f3cf873a120671d64ca422" and
  .candidate_sha == "7fe04d03d5c16c45381aff23eec4e8c6c441ec31" and
  .ciphertext_sha256 == "5f989cbc9a674a4c4e2ee4eb7eea26e0ae1b7dc30026f3a10d042aae138e0859" and
  (.ciphertext_base64 | type == "string" and test("^[A-Za-z0-9+/]+={0,2}$"))
' "$request_file" >/dev/null

work="$(mktemp -d /tmp/zed-vscode-encrypted-publisher.XXXXXX)"
request_id="$(jq -er '.recipient_id' "$request_file")"
private_key="/var/lib/oresoftware/ephemeral-pat-recipients/${request_id}.pem"
metadata="/var/lib/oresoftware/ephemeral-pat-recipients/${request_id}.json"
plain_token="$work/token"
ciphertext="$work/token.enc"
askpass="$work/git-askpass"

cleanup() {
  local rc=$?
  unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN token_value
  rm -f "$private_key" "$metadata"
  find "$work" -type f -exec sh -c 'for f do : > "$f"; done' sh {} + 2>/dev/null || true
  rm -rf "$work"
  exit "$rc"
}
trap cleanup EXIT INT TERM

[[ -f "$private_key" && ! -L "$private_key" ]]
[[ -f "$metadata" && ! -L "$metadata" ]]
[[ "$(stat -c '%u' "$private_key")" == 0 ]]
mode="$(stat -c '%a' "$private_key")"
(( (8#$mode & 0077) == 0 ))
now="$(date +%s)"
expires_at="$(jq -er '.expires_at | floor' "$metadata")"
(( expires_at > now ))
[[ "$(jq -er '.request_id' "$metadata")" == "$request_id" ]]
expected_fingerprint="$(jq -er '.recipient_fingerprint' "$request_file")"
observed_fingerprint="$(openssl pkey -in "$private_key" -pubout 2>/dev/null | openssl pkey -pubin -outform DER 2>/dev/null | sha256sum | awk '{print $1}')"
[[ "$observed_fingerprint" == "$expected_fingerprint" ]]

jq -er '.ciphertext_base64' "$request_file" | base64 --decode > "$ciphertext"
printf '%s  %s\n' "$(jq -er '.ciphertext_sha256' "$request_file")" "$ciphertext" | sha256sum --check --strict
openssl pkeyutl -decrypt -inkey "$private_key" -pkeyopt rsa_padding_mode:oaep -pkeyopt rsa_oaep_md:sha256 -in "$ciphertext" -out "$plain_token"
token_value="$(cat "$plain_token")"
: > "$plain_token"
[[ "$token_value" =~ ^(gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})$ ]]
export GH_TOKEN="$token_value"
export GITHUB_TOKEN="$token_value"
export GITHUB_REPOSITORY_ADMIN_TOKEN="$token_value"
unset token_value

gh auth status >/dev/null
test "$(gh api user --jq .login)" = ORESoftware
membership="$(gh api user/memberships/orgs/zed-pkg)"
test "$(jq -er .state <<<"$membership")" = active
test "$(jq -er .role <<<"$membership")" = admin

cat > "$askpass" <<'ASKPASS'
#!/usr/bin/env bash
case "${1:-}" in
  *Username*) printf '%s\n' x-access-token ;;
  *Password*) printf '%s\n' "${GH_TOKEN:?}" ;;
  *) printf '\n' ;;
esac
ASKPASS
chmod 700 "$askpass"
export GIT_ASKPASS="$askpass"
export GIT_TERMINAL_PROMPT=0

source_repo="$(jq -er '.source_repository' "$request_file")"
source_commit="$(jq -er '.source_commit' "$request_file")"
source="$work/source"
git init -q "$source"
git -C "$source" remote add origin "https://github.com/${source_repo}.git"
git -C "$source" fetch -q --depth=1 origin "$source_commit"
git -C "$source" checkout -q --detach FETCH_HEAD
test "$(git -C "$source" rev-parse HEAD)" = "$source_commit"

bundle="$work/zed-vscode.bundle"
cat "$source"/bootstrap/zed-vscode-20260805.bundle.b64.part-* | base64 --decode > "$bundle"
printf '%s  %s\n' "$(jq -er '.bundle_sha256' "$request_file")" "$bundle" | sha256sum --check --strict
git bundle verify "$bundle" >/dev/null
initial_sha="$(jq -er '.initial_sha' "$request_file")"
candidate_sha="$(jq -er '.candidate_sha' "$request_file")"
test "$(git ls-remote "$bundle" refs/heads/main | cut -f1)" = "$initial_sha"
test "$(git ls-remote "$bundle" refs/heads/dev | cut -f1)" = "$initial_sha"
test "$(git ls-remote "$bundle" refs/heads/agent/den-2278-cross-system-tracking | cut -f1)" = "$candidate_sha"

repo="zed-pkg/zed-vscode"
if ! gh api "repos/$repo" >/dev/null 2>&1; then
  gh api --method POST orgs/zed-pkg/repos \
    -f name=zed-vscode \
    -f description='Visual Studio Code extension for package state, diagnostics, and safe remediation in the independent zed-pkg package manager.' \
    -F private=false -F has_issues=true -F has_projects=true -F has_wiki=false -F auto_init=false >/dev/null
fi
gh api --method PATCH "repos/$repo" \
  -f description='Visual Studio Code extension for package state, diagnostics, and safe remediation in the independent zed-pkg package manager.' \
  -f homepage='https://zpkg.tech' -F has_issues=true -F has_projects=true -F has_wiki=false >/dev/null
printf '%s\n' '{"names":["zed-pkg","vscode-extension","dependency-management","developer-tools","typescript"]}' > "$work/topics.json"
gh api --method PUT "repos/$repo/topics" --input "$work/topics.json" >/dev/null

checkout="$work/checkout"
git init -q "$checkout"
git -C "$checkout" fetch -q "$bundle" \
  refs/heads/main:refs/heads/main \
  refs/heads/dev:refs/heads/dev \
  refs/heads/agent/den-2278-cross-system-tracking:refs/heads/agent/den-2278-cross-system-tracking
git -C "$checkout" checkout -q main
git -C "$checkout" remote add origin "https://github.com/${repo}.git"
git -C "$checkout" push -u origin main dev agent/den-2278-cross-system-tracking
test "$(git -C "$checkout" ls-remote origin refs/heads/main | cut -f1)" = "$initial_sha"
test "$(git -C "$checkout" ls-remote origin refs/heads/dev | cut -f1)" = "$initial_sha"
test "$(git -C "$checkout" ls-remote origin refs/heads/agent/den-2278-cross-system-tracking | cut -f1)" = "$candidate_sha"
gh api --method PATCH "repos/$repo" -f default_branch=main >/dev/null

gh api --method PUT "repos/$repo/actions/permissions" -F enabled=true -f allowed_actions=all >/dev/null || true

pr_number="$(gh pr list --repo "$repo" --head agent/den-2278-cross-system-tracking --base main --state all --json number --jq '.[0].number // empty')"
if [[ -z "$pr_number" ]]; then
  cat > "$work/pr.md" <<'PR'
## Summary

Aligns the VS Code extension with the canonical read-only IDE inspection contract and records cross-system delivery metadata.

- Linear: DEN-2278
- Related protocol work: DEN-2175
- GitHub Project: `zed-pkg-project`
- Candidate: `7fe04d03d5c16c45381aff23eec4e8c6c441ec31`

Diagnostics remain read-only. Every mutation recommendation displays the executable, argument vector, and working directory and requires explicit confirmation.
PR
  pr_url="$(gh pr create --repo "$repo" --head agent/den-2278-cross-system-tracking --base main --title 'docs: align IDE contract and project tracking' --body-file "$work/pr.md")"
  pr_number="${pr_url##*/}"
fi
pr_url="https://github.com/${repo}/pull/${pr_number}"

for _ in $(seq 1 90); do
  total="$(gh api -H 'Accept: application/vnd.github+json' "repos/$repo/commits/$candidate_sha/check-runs" --jq '.total_count')"
  (( total > 0 )) && break
  sleep 10
done
total="$(gh api -H 'Accept: application/vnd.github+json' "repos/$repo/commits/$candidate_sha/check-runs" --jq '.total_count')"
(( total > 0 ))
gh pr checks "$pr_number" --repo "$repo" --watch --interval 10 --fail-fast
test "$(gh pr view "$pr_number" --repo "$repo" --json headRefOid --jq .headRefOid)" = "$candidate_sha"
if [[ -z "$(gh pr view "$pr_number" --repo "$repo" --json mergedAt --jq '.mergedAt // empty')" ]]; then
  merge_result="$(gh api --method PUT "repos/$repo/pulls/$pr_number/merge" -f sha="$candidate_sha" -f merge_method=merge)"
  test "$(jq -r .merged <<<"$merge_result")" = true
fi
merge_sha="$(gh pr view "$pr_number" --repo "$repo" --json mergeCommit --jq .mergeCommit.oid)"
[[ "$merge_sha" =~ ^[0-9a-f]{40}$ ]]

git -C "$checkout" fetch -q origin main
if ! git -C "$checkout" ls-remote --exit-code --tags origin refs/tags/v0.1.0 >/dev/null 2>&1; then
  git -C "$checkout" tag -a v0.1.0 "$merge_sha" -m 'Zed Package Insights 0.1.0'
  git -C "$checkout" push origin v0.1.0
fi
release_run=''
for _ in $(seq 1 90); do
  release_run="$(gh run list --repo "$repo" --workflow Release --limit 30 --json databaseId,headSha --jq ".[] | select(.headSha == \"$merge_sha\") | .databaseId" | head -n1)"
  [[ -n "$release_run" ]] && break
  sleep 10
done
test -n "$release_run"
gh run watch "$release_run" --repo "$repo" --exit-status
mkdir -p "$work/release"
gh release download v0.1.0 --repo "$repo" --pattern '*.vsix' --dir "$work/release"
vsix="$(find "$work/release" -maxdepth 1 -type f -name '*.vsix' -print -quit)"
test -n "$vsix"
vsix_sha="$(sha256sum "$vsix" | awk '{print $1}')"
asset_json="$(gh api "repos/$repo/releases/tags/v0.1.0" --jq '.assets[] | select(.name | endswith(".vsix"))')"
asset_id="$(jq -er .id <<<"$asset_json")"
asset_url="$(jq -er .browser_download_url <<<"$asset_json")"

project_url=''
project_number="$(gh project list --owner zed-pkg --format json --jq '.projects[] | select(.title == "zed-pkg-project") | .number' | head -n1 || true)"
if [[ -z "$project_number" ]]; then
  gh project create --owner zed-pkg --title zed-pkg-project >/dev/null
  project_number="$(gh project list --owner zed-pkg --format json --jq '.projects[] | select(.title == "zed-pkg-project") | .number' | head -n1)"
fi
project_id="$(gh project view "$project_number" --owner zed-pkg --format json --jq .id)"
for field in Repository Linear 'Pull Request' 'Candidate SHA' 'Merge SHA' Gate Evidence; do
  field_id="$(gh project field-list "$project_number" --owner zed-pkg --format json --jq ".fields[] | select(.name == \"$field\") | .id" | head -n1 || true)"
  [[ -n "$field_id" ]] || gh project field-create "$project_number" --owner zed-pkg --name "$field" --data-type TEXT >/dev/null
done
item_id="$(gh project item-list "$project_number" --owner zed-pkg --limit 1000 --format json --jq '.items[] | select(.content.url == "https://github.com/zed-pkg/.github/issues/15") | .id' | head -n1 || true)"
[[ -n "$item_id" ]] || item_id="$(gh project item-add "$project_number" --owner zed-pkg --url https://github.com/zed-pkg/.github/issues/15 --format json --jq .id)"
set_text() {
  local name="$1" value="$2" field_id
  field_id="$(gh project field-list "$project_number" --owner zed-pkg --format json --jq ".fields[] | select(.name == \"$name\") | .id" | head -n1)"
  gh project item-edit --id "$item_id" --project-id "$project_id" --field-id "$field_id" --text "$value" >/dev/null
}
set_text Repository "$repo"
set_text Linear DEN-2278
set_text 'Pull Request' "$pr_url"
set_text 'Candidate SHA' "$candidate_sha"
set_text 'Merge SHA' "$merge_sha"
set_text Gate 'Done: exact-head CI passed, PR merged, and VSIX release artifact published.'
set_text Evidence "release run $release_run; VSIX asset $asset_id; sha256:$vsix_sha"
status_field="$(gh project field-list "$project_number" --owner zed-pkg --format json --jq '.fields[] | select(.name == "Status") | .id' | head -n1 || true)"
done_option="$(gh project field-list "$project_number" --owner zed-pkg --format json --jq '.fields[] | select(.name == "Status") | .options[] | select(.name == "Done") | .id' | head -n1 || true)"
if [[ -n "$status_field" && -n "$done_option" ]]; then
  gh project item-edit --id "$item_id" --project-id "$project_id" --field-id "$status_field" --single-select-option-id "$done_option" >/dev/null
fi
project_url="$(gh project view "$project_number" --owner zed-pkg --format json --jq .url)"

cat > "$work/tracking.md" <<EOF
Published and merged the dedicated **zed-vscode** extension repository.

- Repository: https://github.com/$repo
- Pull request: $pr_url
- Candidate SHA: \`$candidate_sha\`
- Merge SHA: \`$merge_sha\`
- Release: https://github.com/$repo/releases/tag/v0.1.0
- Release workflow run: \`$release_run\`
- VSIX asset ID: \`$asset_id\`
- VSIX SHA-256: \`$vsix_sha\`
- GitHub Project: $project_url
- Linear: DEN-2278
EOF
gh issue comment 15 --repo zed-pkg/.github --body-file "$work/tracking.md"
gh issue close 15 --repo zed-pkg/.github --reason completed

printf 'ZED_VSCODE_PUBLICATION_COMPLETE=true\n'
printf 'ZED_VSCODE_REPOSITORY=https://github.com/%s\n' "$repo"
printf 'ZED_VSCODE_PULL_REQUEST=%s\n' "$pr_url"
printf 'ZED_VSCODE_CANDIDATE_SHA=%s\n' "$candidate_sha"
printf 'ZED_VSCODE_MERGE_SHA=%s\n' "$merge_sha"
printf 'ZED_VSCODE_RELEASE_RUN=%s\n' "$release_run"
printf 'ZED_VSCODE_VSIX_ASSET_ID=%s\n' "$asset_id"
printf 'ZED_VSCODE_VSIX_URL=%s\n' "$asset_url"
printf 'ZED_VSCODE_VSIX_SHA256=%s\n' "$vsix_sha"
printf 'ZED_VSCODE_PROJECT_NUMBER=%s\n' "$project_number"
printf 'ZED_VSCODE_PROJECT_URL=%s\n' "$project_url"
printf 'ZED_VSCODE_TOKEN_EXPOSED=false\n'

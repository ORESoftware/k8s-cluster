#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly TARGET_REPOSITORY='ORESoftware/mcp-rust-libs'
readonly SOURCE_REPOSITORY='ORESoftware/testing'
readonly SOURCE_SHA='069b1aa4251658c8348d2eb477ad71369d9b742b'
readonly SOURCE_SUBDIRECTORY='mcp-rust-libs'
readonly SOURCE_MANIFEST_SHA256='b9ba89f29dca3e5020430d3a5d35967e523d3e94db9168a91cdf24a9bd5f2a33'
readonly BOOTSTRAP_BRANCH='bootstrap/semantic-polyglot-20260731'
readonly BASELINE_TIMESTAMP='2026-07-31T13:30:00Z'
readonly SOURCE_TIMESTAMP='2026-07-31T13:31:00Z'
readonly MAX_CHECK_POLLS=720
readonly CHECK_POLL_SECONDS=10

stage=bootstrap
work=''
report_failure() {
  local status=$?
  trap - ERR
  printf 'publisher-stage-failed=%s exit=%s\n' "$stage" "$status" >&2
  exit "$status"
}
cleanup() {
  unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN \
    GIT_ASKPASS GIT_ASKPASS_REQUIRE GIT_TERMINAL_PROMPT
  if [[ -n "$work" ]]; then
    rm -rf "$work"
  fi
}
trap report_failure ERR
trap cleanup EXIT

: "${GH_TOKEN:?GH_TOKEN must contain the masked AWS-backed ORESoftware credential}"
if [[ "$GH_TOKEN" == *$'\n'* || "$GH_TOKEN" == *$'\r'* || "$GH_TOKEN" == *$'\t'* || "$GH_TOKEN" == *' '* ]]; then
  echo 'publisher credential contains whitespace' >&2
  exit 65
fi
export GITHUB_REPOSITORY_ADMIN_TOKEN="$GH_TOKEN"

work="$(mktemp -d /tmp/mcp-rust-libs-actions-publisher.XXXXXX)"

stage=verify-github-identity
actual_login="$(gh api user --jq .login)"
test "$actual_login" = ORESoftware
printf 'publisher-stage=%s status=ready actor=%s\n' "$stage" "$actual_login"

stage=configure-git-authentication
askpass="$work/git-askpass.sh"
cat > "$askpass" <<'ASKPASS'
#!/usr/bin/env sh
case "${1:-}" in
  *Username*) printf '%s\n' x-access-token ;;
  *Password*) printf '%s\n' "${GH_TOKEN:?}" ;;
  *) exit 1 ;;
esac
ASKPASS
chmod 700 "$askpass"
export GIT_ASKPASS="$askpass"
export GIT_ASKPASS_REQUIRE=force
export GIT_TERMINAL_PROMPT=0

stage=checkout-reviewed-source
carrier="$work/carrier"
git clone --filter=blob:none --no-checkout "https://github.com/${SOURCE_REPOSITORY}.git" "$carrier"
git -C "$carrier" fetch --depth=1 origin "$SOURCE_SHA"
git -C "$carrier" checkout --detach "$SOURCE_SHA"
test "$(git -C "$carrier" rev-parse HEAD)" = "$SOURCE_SHA"
source_root="$carrier/$SOURCE_SUBDIRECTORY"
for required in \
  Cargo.toml \
  .zpkg.toml \
  .github/workflows/scaffold.yml \
  packages/rust/Cargo.toml \
  packages/typescript/package.json \
  packages/dart/pubspec.yaml \
  packages/gleam/gleam.toml \
  reports/polyglot-api-repair-complete.txt \
  reports/source-files.sha256; do
  test -f "$source_root/$required"
done
test "$(sha256sum "$source_root/reports/source-files.sha256" | awk '{print $1}')" = \
  "$SOURCE_MANIFEST_SHA256"
source_tree="$(git -C "$carrier" rev-parse "${SOURCE_SHA}:${SOURCE_SUBDIRECTORY}")"
[[ "$source_tree" =~ ^[0-9a-f]{40}$ ]]

stage=validate-reviewed-source
(
  cd "$source_root"
  python3 scripts/regenerate-generated.py --check
  python3 scripts/static-source-checks.py
  python3 scripts/check-scaffold.py
  python3 tooling/conformance/run.py
  python3 scripts/update-source-manifest.py --check
)

stage=ensure-target-repository
repo_json="$(gh api "repos/${TARGET_REPOSITORY}" 2>/dev/null || true)"
if [[ -z "$repo_json" ]]; then
  repo_json="$(
    gh api --method POST user/repos \
      -f name='mcp-rust-libs' \
      -f description='Shared Rust, TypeScript, Dart, and Gleam MCP runtime, contracts, safety, telemetry, code generation, and testkit libraries' \
      -F private=false \
      -F has_issues=true \
      -F has_projects=false \
      -F has_wiki=false \
      -F auto_init=false
  )"
  echo "CREATED ${TARGET_REPOSITORY}"
fi
test "$(jq -r .owner.login <<<"$repo_json")" = ORESoftware
test "$(jq -r .visibility <<<"$repo_json")" = public
gh api --method PATCH "repos/${TARGET_REPOSITORY}" \
  -f description='Shared Rust, TypeScript, Dart, and Gleam MCP runtime, contracts, safety, telemetry, code generation, and testkit libraries' \
  -F private=false \
  -F has_issues=true \
  -F has_projects=false \
  -F has_wiki=false \
  -F allow_squash_merge=true \
  -F allow_merge_commit=true \
  -F allow_rebase_merge=false \
  -F delete_branch_on_merge=true >/dev/null

stage=prepare-deterministic-baseline
baseline="$work/baseline"
mkdir -p "$baseline/.github/workflows"
git -C "$baseline" init -b main
git -C "$baseline" config user.name 'ORESoftware publication automation'
git -C "$baseline" config user.email 'bot@oresoftware.dev'
cp "$source_root/README.md" "$baseline/README.md"
cp "$source_root/LICENSE-MIT" "$baseline/LICENSE-MIT"
cp "$source_root/LICENSE-APACHE" "$baseline/LICENSE-APACHE"
if [[ -f "$source_root/CODEOWNERS" ]]; then
  cp "$source_root/CODEOWNERS" "$baseline/CODEOWNERS"
fi
if [[ -f "$source_root/.github/CODEOWNERS" ]]; then
  mkdir -p "$baseline/.github"
  cp "$source_root/.github/CODEOWNERS" "$baseline/.github/CODEOWNERS"
fi
cp "$source_root/.github/workflows/scaffold.yml" "$baseline/.github/workflows/scaffold.yml"
git -C "$baseline" add -A
GIT_AUTHOR_DATE="$BASELINE_TIMESTAMP" GIT_COMMITTER_DATE="$BASELINE_TIMESTAMP" \
  git -C "$baseline" commit -m 'chore: initialize canonical repository and review gate [skip ci]'
expected_baseline_tree="$(git -C "$baseline" rev-parse 'HEAD^{tree}')"

remote_main="$(gh api "repos/${TARGET_REPOSITORY}/git/ref/heads/main" --jq .object.sha 2>/dev/null || true)"
if [[ -z "$remote_main" ]]; then
  git -C "$baseline" remote add origin "https://github.com/${TARGET_REPOSITORY}.git"
  git -C "$baseline" -c credential.helper= push origin HEAD:refs/heads/main
  remote_main="$(gh api "repos/${TARGET_REPOSITORY}/git/ref/heads/main" --jq .object.sha)"
  gh api --method PATCH "repos/${TARGET_REPOSITORY}" -f default_branch=main >/dev/null
fi

stage=verify-target-main-state
target="$work/target"
git clone --depth=1 --branch main "https://github.com/${TARGET_REPOSITORY}.git" "$target"
main_tree="$(git -C "$target" rev-parse 'HEAD^{tree}')"
if [[ "$main_tree" = "$source_tree" ]]; then
  echo "ALREADY_PUBLISHED ${TARGET_REPOSITORY} main=${remote_main} source_tree=${source_tree}"
  exit 0
fi
if [[ "$main_tree" != "$expected_baseline_tree" ]]; then
  echo "unexpected target main tree: ${main_tree}" >&2
  exit 1
fi

stage=publish-exact-bootstrap-branch
remote_head="$(git -C "$target" ls-remote origin "refs/heads/${BOOTSTRAP_BRANCH}" | awk '{print $1}')"
if [[ -n "$remote_head" ]]; then
  git -C "$target" fetch --depth=1 origin "$BOOTSTRAP_BRANCH"
  branch_tree="$(git -C "$target" rev-parse 'FETCH_HEAD^{tree}')"
  test "$branch_tree" = "$source_tree"
  expected_head="$remote_head"
else
  git -C "$target" checkout -b "$BOOTSTRAP_BRANCH" main
  find "$target" -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf {} +
  cp -a "$source_root"/. "$target"/
  git -C "$target" config user.name 'ORESoftware publication automation'
  git -C "$target" config user.email 'bot@oresoftware.dev'
  git -C "$target" add -A
  GIT_AUTHOR_DATE="$SOURCE_TIMESTAMP" GIT_COMMITTER_DATE="$SOURCE_TIMESTAMP" \
    git -C "$target" commit \
      -m 'feat: bootstrap shared polyglot MCP libraries' \
      -m "Promote reviewed ${SOURCE_REPOSITORY}@${SOURCE_SHA} subtree to canonical repository root."
  expected_head="$(git -C "$target" rev-parse HEAD)"
  test "$(git -C "$target" rev-parse 'HEAD^{tree}')" = "$source_tree"
  git -C "$target" -c credential.helper= push origin "HEAD:refs/heads/${BOOTSTRAP_BRANCH}"
fi
test "$(gh api "repos/${TARGET_REPOSITORY}/git/ref/heads/${BOOTSTRAP_BRANCH}" --jq .object.sha)" = "$expected_head"

stage=ensure-bootstrap-pull-request
pulls="$(gh pr list --repo "$TARGET_REPOSITORY" --head "$BOOTSTRAP_BRANCH" --base main --state all --json number,state,mergedAt,headRefOid)"
test "$(jq length <<<"$pulls")" -le 1
if [[ "$(jq length <<<"$pulls")" -eq 0 ]]; then
  pr_body="$work/bootstrap-pr.md"
  cat > "$pr_body" <<EOF
## Canonical bootstrap

This promotes the exact reviewed \`${SOURCE_SUBDIRECTORY}/\` subtree from
\`${SOURCE_REPOSITORY}@${SOURCE_SHA}\` to repository root.

- source-manifest SHA-256: \`${SOURCE_MANIFEST_SHA256}\`
- source Git tree: \`${source_tree}\`
- bootstrap head: \`${expected_head}\`

Product tools, credentials, authorization, mutation policy, endpoint ownership,
and business schemas remain in their owning repositories.

Merge only after the complete target polyglot matrix succeeds.

Refs DEN-319, DEN-957, DEN-959, DEN-967, DEN-968, DEN-969, DEN-970, DEN-972, DEN-1186.
EOF
  gh pr create \
    --repo "$TARGET_REPOSITORY" \
    --head "$BOOTSTRAP_BRANCH" \
    --base main \
    --title 'Bootstrap shared polyglot MCP libraries' \
    --body-file "$pr_body" >/dev/null
  pulls="$(gh pr list --repo "$TARGET_REPOSITORY" --head "$BOOTSTRAP_BRANCH" --base main --state all --json number,state,mergedAt,headRefOid)"
fi
pr_number="$(jq -r '.[0].number' <<<"$pulls")"
test "$(jq -r '.[0].headRefOid' <<<"$pulls")" = "$expected_head"
if [[ "$(jq -r '.[0].state' <<<"$pulls")" = CLOSED && "$(jq -r '.[0].mergedAt // empty' <<<"$pulls")" = '' ]]; then
  gh pr reopen "$pr_number" --repo "$TARGET_REPOSITORY" >/dev/null
fi
printf 'PUBLISHED %s PR#%s head=%s source_tree=%s\n' \
  "$TARGET_REPOSITORY" "$pr_number" "$expected_head" "$source_tree"

stage=wait-for-target-ci
checks_seen=false
for _ in $(seq 1 "$MAX_CHECK_POLLS"); do
  check_runs="$(gh api "repos/${TARGET_REPOSITORY}/commits/${expected_head}/check-runs?per_page=100")"
  count="$(jq '.total_count' <<<"$check_runs")"
  if (( count > 0 )); then
    checks_seen=true
    incomplete="$(jq '[.check_runs[] | select(.status != "completed")] | length' <<<"$check_runs")"
    failed="$(jq '[.check_runs[] | select(.status == "completed" and (.conclusion | IN("success", "neutral", "skipped") | not))] | length' <<<"$check_runs")"
    if (( failed > 0 )); then
      jq -r '.check_runs[] | select(.status == "completed" and (.conclusion | IN("success", "neutral", "skipped") | not)) | "failed-check=" + .name + " conclusion=" + (.conclusion // "")' <<<"$check_runs" >&2
      exit 1
    fi
    if (( incomplete == 0 )); then
      break
    fi
  fi
  sleep "$CHECK_POLL_SECONDS"
done
if [[ "$checks_seen" != true ]]; then
  echo 'target CI never registered a check run' >&2
  exit 1
fi
final_checks="$(gh api "repos/${TARGET_REPOSITORY}/commits/${expected_head}/check-runs?per_page=100")"
test "$(jq '[.check_runs[] | select(.status != "completed")] | length' <<<"$final_checks")" -eq 0
test "$(jq '[.check_runs[] | select(.conclusion | IN("success", "neutral", "skipped") | not)] | length' <<<"$final_checks")" -eq 0
jq -r '.check_runs[] | "target-check=" + .name + " conclusion=" + (.conclusion // "")' <<<"$final_checks"

stage=merge-bootstrap-pull-request
merge_result="$(
  gh api --method PUT "repos/${TARGET_REPOSITORY}/pulls/${pr_number}/merge" \
    -f sha="$expected_head" \
    -f merge_method=merge \
    -f commit_title='Bootstrap shared polyglot MCP libraries' \
    -f commit_message='Merge the exact reviewed mcp-rust-libs source tree after the complete target CI matrix passed.'
)"
test "$(jq -r .merged <<<"$merge_result")" = true
merge_sha="$(jq -r .sha <<<"$merge_result")"
[[ "$merge_sha" =~ ^[0-9a-f]{40}$ ]]

gh api --method DELETE "repos/${TARGET_REPOSITORY}/git/refs/heads/${BOOTSTRAP_BRANCH}" >/dev/null 2>&1 || true

stage=verify-canonical-main
git -C "$target" fetch --depth=1 origin main
canonical_main="$(git -C "$target" rev-parse FETCH_HEAD)"
canonical_tree="$(git -C "$target" rev-parse 'FETCH_HEAD^{tree}')"
test "$canonical_main" = "$merge_sha"
test "$canonical_tree" = "$source_tree"
repo_json="$(gh api "repos/${TARGET_REPOSITORY}")"
test "$(jq -r .owner.login <<<"$repo_json")" = ORESoftware
test "$(jq -r .visibility <<<"$repo_json")" = public
test "$(jq -r .default_branch <<<"$repo_json")" = main

stage=protect-canonical-main
protection="$work/protection.json"
cat > "$protection" <<'JSON'
{
  "required_status_checks": null,
  "enforce_admins": false,
  "required_pull_request_reviews": {
    "dismiss_stale_reviews": true,
    "require_code_owner_reviews": false,
    "required_approving_review_count": 0
  },
  "restrictions": null,
  "required_linear_history": false,
  "allow_force_pushes": false,
  "allow_deletions": false,
  "block_creations": false,
  "required_conversation_resolution": true,
  "lock_branch": false,
  "allow_fork_syncing": true
}
JSON
gh api --method PUT "repos/${TARGET_REPOSITORY}/branches/main/protection" --input "$protection" >/dev/null

stage=complete
printf 'CANONICAL_REPOSITORY=%s\n' "$TARGET_REPOSITORY"
printf 'CANONICAL_MAIN=%s\n' "$canonical_main"
printf 'CANONICAL_TREE=%s\n' "$canonical_tree"
printf 'SOURCE_TREE=%s\n' "$source_tree"
printf 'BOOTSTRAP_PR=%s\n' "$pr_number"
printf 'TARGET_VISIBILITY=public\n'
printf 'TARGET_DEFAULT_BRANCH=main\n'

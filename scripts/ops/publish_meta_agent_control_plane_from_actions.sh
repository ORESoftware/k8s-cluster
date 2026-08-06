#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly SOURCE_SHA="55ee15c190b7cfa4e075f6984c7cb551acd4b9d3"
readonly BUNDLE_SHA256="1ddaa03743b864348162149b7d2d2e2dce7eab585cf092ea14547c647fcec031"
readonly PUBLISHER_SHA256="e2fe6eaa622db02a54f83e27a822f64ad4b54971c883f97bbda4ac0a4db5d278"
readonly TARGET_REPOSITORY="meta-agents-demo/meta-agent-control-plane.rs"
readonly EXPECTED_MAIN="4d6ec3ad0ec7b688f0e777129eee7e0f0d999df1"
readonly FEATURE_REF="agent/den-1057-meta-agent-control-plane"
readonly EXPECTED_FEATURE="789d48039da232faed985d4f8de176959f117e08"

fail() {
  printf 'meta-agent-publication-error: %s\n' "$1" >&2
  exit 1
}

[[ "${SOURCE_SHA}" =~ ^[0-9a-f]{40}$ ]] || fail "invalid source SHA"
[[ "${EXPECTED_MAIN}" =~ ^[0-9a-f]{40}$ ]] || fail "invalid expected main SHA"
[[ "${EXPECTED_FEATURE}" =~ ^[0-9a-f]{40}$ ]] || fail "invalid expected feature SHA"

token="${GITHUB_REPOSITORY_ADMIN_TOKEN:-${GH_TOKEN:-}}"
test -n "$token" || fail "repository-administration credential is missing"
[[ "$token" != *$'\n'* && "$token" != *$'\r'* && "$token" != *$'\t'* && "$token" != *' '* ]] \
  || fail "repository-administration credential contains whitespace"
export GH_TOKEN="$token"
export GITHUB_REPOSITORY_ADMIN_TOKEN="$token"
unset token

actor="$(gh api user --jq .login)"
test "$actor" = ORESoftware || fail "credential does not authenticate as ORESoftware"

membership="$(gh api user/memberships/orgs/meta-agents-demo)"
test "$(jq -er .role <<<"$membership")" = admin \
  || fail "ORESoftware is not a meta-agents-demo administrator"
test "$(jq -er .state <<<"$membership")" = active \
  || fail "ORESoftware meta-agents-demo membership is not active"
unset membership

work="$(mktemp -d "${RUNNER_TEMP:-/tmp}/meta-agent-publication.XXXXXX")"
cleanup() {
  unset GH_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN GIT_ASKPASS GIT_TERMINAL_PROMPT
  rm -rf "$work"
}
trap cleanup EXIT

git cat-file -e "${SOURCE_SHA}^{commit}" \
  || fail "pinned source commit is unavailable in the trusted checkout"

mapfile -t bundle_parts < <(
  git ls-tree -r --name-only "$SOURCE_SHA" -- scripts/critical-org-fleet/assets \
    | grep '^scripts/critical-org-fleet/assets/meta\.part' \
    | LC_ALL=C sort
)
((${#bundle_parts[@]} > 0)) || fail "pinned Meta Agents bundle parts are missing"

bundle="$work/meta-agent-control-plane-den-1057.bundle"
{
  for part in "${bundle_parts[@]}"; do
    git show "${SOURCE_SHA}:${part}"
  done
} | base64 --decode > "$bundle"
printf '%s  %s\n' "$BUNDLE_SHA256" "$bundle" | sha256sum --check --strict

test "$(git bundle list-heads "$bundle" | wc -l)" -eq 2 \
  || fail "pinned bundle does not contain exactly two reviewed refs"
git bundle verify "$bundle" >/dev/null

publisher="$work/publish_meta_control_plane.py"
git show "${SOURCE_SHA}:scripts/critical-org-fleet/publish_meta_control_plane.py" > "$publisher"
printf '%s  %s\n' "$PUBLISHER_SHA256" "$publisher" | sha256sum --check --strict
python3 -m py_compile "$publisher"

python3 "$publisher" "$bundle"

repo="$(gh api "repos/${TARGET_REPOSITORY}")"
test "$(jq -er .visibility <<<"$repo")" = public \
  || fail "created repository is not public"
test "$(jq -er .default_branch <<<"$repo")" = main \
  || fail "created repository default branch is not main"
unset repo

observed_main="$(gh api "repos/${TARGET_REPOSITORY}/git/ref/heads/main" --jq .object.sha)"
observed_feature="$(
  gh api "repos/${TARGET_REPOSITORY}/git/ref/heads/${FEATURE_REF}" --jq .object.sha
)"
test "$observed_main" = "$EXPECTED_MAIN" \
  || fail "remote main does not match the reviewed bundle"
test "$observed_feature" = "$EXPECTED_FEATURE" \
  || fail "remote feature branch does not match the reviewed bundle"

printf 'created_repository=%s\n' "$TARGET_REPOSITORY"
printf 'verified_main=%s\n' "$observed_main"
printf 'verified_feature=%s\n' "$observed_feature"

#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
manifest="$root/manifest.json"
bundle=''
output=''
execute=0
base_branch='main'
feature_branch='agent/bootstrap-apostille-me'

usage() {
  echo "usage: $0 --bundle PATH --output PATH [--execute]" >&2
  exit 2
}

while (($#)); do
  case "$1" in
    --bundle) bundle="${2:?missing bundle path}"; shift 2 ;;
    --output) output="${2:?missing output path}"; shift 2 ;;
    --execute) execute=1; shift ;;
    *) usage ;;
  esac
done

test -n "$bundle" && test -n "$output" || usage
bundle="$(realpath "$bundle")"
output="$(realpath -m "$output")"

for command in git jq sha256sum; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "required command is missing: $command" >&2
    exit 2
  }
done
if ((execute)); then
  command -v gh >/dev/null 2>&1 || { echo 'required command is missing: gh' >&2; exit 2; }
  case "${GH_TOKEN:-}" in
    gho_*) ;;
    *) echo 'execution requires a temporary gho_ device-OAuth credential' >&2; exit 65 ;;
  esac
fi

schema="$(jq -r .schema_version "$manifest")"
org="$(jq -r .organization "$manifest")"
expected_login="$(jq -r .expected_login "$manifest")"
visibility="$(jq -r .visibility "$manifest")"
manifest_base="$(jq -r .default_branch "$manifest")"
manifest_feature="$(jq -r .feature_branch "$manifest")"
expected_bundle_sha="$(jq -r .bundle_sha256 "$manifest")"
repo_count="$(jq '.repositories | length' "$manifest")"

test "$schema" = 1
test "$org" = apostille-me
test "$expected_login" = ORESoftware
test "$visibility" = public
test "$manifest_base" = "$base_branch"
test "$manifest_feature" = "$feature_branch"
test "$repo_count" = 8
[[ "$expected_bundle_sha" =~ ^[0-9a-f]{64}$ ]]
test -f "$bundle"
echo "$expected_bundle_sha  $bundle" | sha256sum --check --strict

mapfile -t repository_rows < <(jq -c '.repositories[]' "$manifest")
declare -A seen_names=()
declare -A observed_refs=()
while read -r sha ref; do
  test -n "$sha" && test -n "$ref"
  test -z "${observed_refs[$ref]+x}" || { echo "duplicate bundle ref: $ref" >&2; exit 4; }
  observed_refs["$ref"]="$sha"
done < <(git bundle list-heads "$bundle")

expected_ref_count=0
for row in "${repository_rows[@]}"; do
  name="$(jq -r .name <<<"$row")"
  main_sha="$(jq -r .main_sha <<<"$row")"
  feature_sha="$(jq -r .feature_sha <<<"$row")"
  tag="$(jq -r '.tag // empty' <<<"$row")"
  tag_ref_sha="$(jq -r '.tag_ref_sha // empty' <<<"$row")"
  [[ "$name" =~ ^apme-[a-z0-9-]+$ ]]
  [[ "$main_sha" =~ ^[0-9a-f]{40}$ ]]
  [[ "$feature_sha" =~ ^[0-9a-f]{40}$ ]]
  test -z "${seen_names[$name]+x}" || { echo "duplicate repository: $name" >&2; exit 4; }
  seen_names["$name"]=1
  main_ref="refs/apme/$name/main"
  feature_ref="refs/apme/$name/feature"
  test "${observed_refs[$main_ref]:-}" = "$main_sha"
  test "${observed_refs[$feature_ref]:-}" = "$feature_sha"
  expected_ref_count=$((expected_ref_count + 2))
  if test -n "$tag"; then
    [[ "$tag_ref_sha" =~ ^[0-9a-f]{40}$ ]]
    test "${observed_refs[refs/apme/$name/tags/$tag]:-}" = "$tag_ref_sha"
    expected_ref_count=$((expected_ref_count + 1))
  fi
done
test "${#observed_refs[@]}" = "$expected_ref_count"

work="$(mktemp -d /tmp/apostille-me-publisher.XXXXXX)"
cleanup() {
  rm -rf "$work"
  unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN
}
trap cleanup EXIT

if ((execute)); then
  test "$(gh api user --jq .login)" = "$expected_login"
  membership="$(gh api "user/memberships/orgs/$org" --jq '[.role,.state] | @tsv')"
  test "$membership" = $'admin\tactive'
fi

results="$work/results.jsonl"
: > "$results"

push_exact_branch() {
  local repo_dir="$1" remote="$2" branch="$3" expected_sha="$4" existing
  existing="$(git -C "$repo_dir" ls-remote --heads "$remote" "refs/heads/$branch" | awk 'NR == 1 {print $1}')"
  if test -n "$existing" && test "$existing" != "$expected_sha"; then
    echo "remote branch diverged: $remote $branch expected=$expected_sha observed=$existing" >&2
    exit 4
  fi
  git -C "$repo_dir" push --porcelain "$remote" "refs/heads/$branch:refs/heads/$branch"
}

for row in "${repository_rows[@]}"; do
  name="$(jq -r .name <<<"$row")"
  description="$(jq -r .description <<<"$row")"
  title="$(jq -r .title <<<"$row")"
  main_sha="$(jq -r .main_sha <<<"$row")"
  feature_sha="$(jq -r .feature_sha <<<"$row")"
  tag="$(jq -r '.tag // empty' <<<"$row")"
  tag_ref_sha="$(jq -r '.tag_ref_sha // empty' <<<"$row")"
  tag_target_sha="$(jq -r '.tag_target_sha // empty' <<<"$row")"

  repo_dir="$work/$name"
  git init --quiet "$repo_dir"
  git -C "$repo_dir" remote add bundle "$bundle"
  fetch_specs=(
    "refs/apme/$name/main:refs/heads/$base_branch"
    "refs/apme/$name/feature:refs/heads/$feature_branch"
  )
  if test -n "$tag"; then
    fetch_specs+=("refs/apme/$name/tags/$tag:refs/tags/$tag")
  fi
  git -C "$repo_dir" fetch --quiet bundle "${fetch_specs[@]}"
  git -C "$repo_dir" remote remove bundle
  git -C "$repo_dir" fsck --full --strict
  test "$(git -C "$repo_dir" rev-parse "$base_branch")" = "$main_sha"
  test "$(git -C "$repo_dir" rev-parse "$feature_branch")" = "$feature_sha"
  test -z "$(git -C "$repo_dir" ls-tree -r --name-only "$main_sha")"
  git -C "$repo_dir" merge-base --is-ancestor "$main_sha" "$feature_sha"
  test "$(git -C "$repo_dir" rev-list --count "$main_sha..$feature_sha")" = 1
  if test -n "$tag"; then
    test "$(git -C "$repo_dir" rev-parse "$tag")" = "$tag_ref_sha"
    test "$(git -C "$repo_dir" rev-parse "$tag^{}")" = "$tag_target_sha"
  fi

  if ((!execute)); then
    continue
  fi

  full="$org/$name"
  remote="https://github.com/$full.git"
  if ! gh api "repos/$full" >/dev/null 2>&1; then
    gh repo create "$full" --public --description "$description" --disable-wiki
  fi
  test "$(gh api "repos/$full" --jq .owner.login)" = "$org"
  test "$(gh api "repos/$full" --jq .visibility)" = "$visibility"

  push_exact_branch "$repo_dir" "$remote" "$base_branch" "$main_sha"
  gh repo edit "$full" --default-branch "$base_branch"
  push_exact_branch "$repo_dir" "$remote" "$feature_branch" "$feature_sha"
  if test -n "$tag"; then
    remote_tag="$(git -C "$repo_dir" ls-remote --tags "$remote" "refs/tags/$tag" | awk 'NR == 1 {print $1}')"
    if test -n "$remote_tag" && test "$remote_tag" != "$tag_ref_sha"; then
      echo "remote tag diverged: $full $tag" >&2
      exit 4
    fi
    git -C "$repo_dir" push --porcelain "$remote" "refs/tags/$tag:refs/tags/$tag"
  fi
  gh repo edit "$full" \
    --enable-issues \
    --enable-squash-merge \
    --delete-branch-on-merge \
    --add-topic apostille \
    --add-topic visa-consulting \
    --add-topic rust

  pr_url="$(gh pr list --repo "$full" --base "$base_branch" --head "$feature_branch" --state all --json url --jq '.[0].url // empty')"
  if test -z "$pr_url"; then
    body="$work/$name-pr.md"
    cat > "$body" <<BODY
## What changed

Bootstraps **$name** for the apostille-me visa and apostille consulting platform.

## Why

This establishes the first reviewable implementation while keeping the default branch free of direct application-code publication.

## Impact

- introduces the requested Rust service or supporting repository
- includes CI, documentation, security defaults, dependency automation, and local-development configuration
- keeps the implementation isolated on \`$feature_branch\` until reviewed and merged

## Validation

- immutable combined Git bundle and expected ref SHA verification
- empty \`main\` review base and exactly one bootstrap commit
- clean Git object integrity check
- repository-specific CI runs on this pull request

Published through a temporary GitHub device-OAuth credential. Personal access tokens are rejected by the workflow.
BODY
    pr_url="$(gh pr create --repo "$full" --base "$base_branch" --head "$feature_branch" --title "$title" --body-file "$body")"
  fi
  pr_json="$(gh pr view "$pr_url" --json number,url,state,isDraft,baseRefName,headRefName)"
  test "$(jq -r .state <<<"$pr_json")" = OPEN
  test "$(jq -r .isDraft <<<"$pr_json")" = false
  test "$(jq -r .baseRefName <<<"$pr_json")" = "$base_branch"
  test "$(jq -r .headRefName <<<"$pr_json")" = "$feature_branch"
  test "$(gh api "repos/$full/git/ref/heads/$base_branch" --jq .object.sha)" = "$main_sha"
  test "$(gh api "repos/$full/git/ref/heads/$feature_branch" --jq .object.sha)" = "$feature_sha"
  repo_url="$(gh api "repos/$full" --jq .html_url)"
  jq -nc \
    --arg repository "$full" \
    --arg repository_url "$repo_url" \
    --arg main_sha "$main_sha" \
    --arg feature_sha "$feature_sha" \
    --argjson pull_request_number "$(jq .number <<<"$pr_json")" \
    --arg pull_request_url "$(jq -r .url <<<"$pr_json")" \
    '{repository:$repository,repository_url:$repository_url,main_sha:$main_sha,feature_sha:$feature_sha,pull_request_number:$pull_request_number,pull_request_url:$pull_request_url}' \
    >> "$results"
  echo "published $full feature=$feature_sha pr=$pr_url"
done

mkdir -p "$(dirname "$output")"
if ((execute)); then
  jq -s \
    --arg organization "$org" \
    '{schema_version:1,status:"success",organization:$organization,repository_count:length,repositories:.}' \
    "$results" > "$output"
  test "$(jq -r .repository_count "$output")" = 8
else
  jq -n \
    --arg organization "$org" \
    --argjson repositories "$(jq '[.repositories[].name]' "$manifest")" \
    '{schema_version:1,status:"validated",organization:$organization,repository_count:($repositories|length),repositories:$repositories}' \
    > "$output"
fi
cat "$output"

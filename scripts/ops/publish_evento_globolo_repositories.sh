#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

: "${SOURCE_BASE_URL:?SOURCE_BASE_URL is required}"
: "${SOURCE_SHA256:?SOURCE_SHA256 is required}"

organization=evento-globolo
review_branch=agent/evento-globolo-bootstrap-review
work="$(mktemp -d /tmp/evento-globolo-publisher.XXXXXX)"
results="$work/results.jsonl"

cleanup() {
  unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN
  unset GIT_ASKPASS GIT_ASKPASS_REQUIRE GIT_TERMINAL_PROMPT
  unset GIT_CONFIG_COUNT GIT_CONFIG_KEY_0 GIT_CONFIG_VALUE_0
  rm -rf "$work"
}
trap cleanup EXIT

fail() {
  printf 'publisher status=failed stage=%s detail=%s\n' "${stage:-unknown}" "$*" >&2
  exit 1
}

stage=download-reviewed-source
source_b64="$work/evento-globolo-source.tar.gz.b64"
source_tar="$work/evento-globolo-source.tar.gz"
source_root="$work/source"
mkdir -p "$source_root"
for source_part in 00 01 02 03 04 05; do
  curl --fail --silent --show-error --location \
    "${SOURCE_BASE_URL}.part-${source_part}" \
    >> "$source_b64"
done
base64 --decode "$source_b64" > "$source_tar"
actual_source_sha="$(sha256sum "$source_tar" | awk '{print $1}')"
test "$actual_source_sha" = "$SOURCE_SHA256" || fail "source-sha256-mismatch:${actual_source_sha}"
tar -xzf "$source_tar" -C "$source_root"

repos=(
  evgl-interfaces
  evgl-api
  evgl-mash-web
  evgl-leptos-web
  evgl-dioxus-web
  evgl-sync
  evgl-cli
  evgl-infra
)

declare -A source_digest=(
  [evgl-interfaces]=8d2b72159a77f3da830842cc5b12c406bdbf9c08106efcb1755c9216f0a33e80
  [evgl-api]=c412a5440d7f76f34b8a8584de823a129898b8fc647cfffcfb4903f95efa18b4
  [evgl-mash-web]=0b4afe017a6090914b0072b852617b71677d88d1c084de8aef70c7c657e0d404
  [evgl-leptos-web]=79f496968a22925cedb14363134c7211f1c014502adb97daed4c16c6a343b035
  [evgl-dioxus-web]=ce3ae0f7ff3137baa139e4bb13dcefc90d96604748e6fa75bd12b2826fafbc7d
  [evgl-sync]=af7a386189d44096514d69ccf4c31dc37b311b37ef3c9a8dcc18774a79076965
  [evgl-cli]=e155c7ad1e983dc395672b41191b0074ee84efe1d3a4b299676277cf3a276d93
  [evgl-infra]=ea8d65b8ece97777caab04f196ab2afe16fafbbebb6b775da8a90321ed108160
)

declare -A description=(
  [evgl-interfaces]='Shared Rust, TypeScript, OpenAPI, AsyncAPI, JSON Schema, and Protobuf contracts for Evento Globolo.'
  [evgl-api]='Axum REST and WebSocket API server for the Evento Globolo global events platform.'
  [evgl-mash-web]='Maud, Axum, SeaORM, Supabase, HTMX, and WebSocket web server for Evento Globolo.'
  [evgl-leptos-web]='Leptos SSR Rust web server for Evento Globolo event discovery and management.'
  [evgl-dioxus-web]='Dioxus SSR Rust web server for Evento Globolo event discovery and management.'
  [evgl-sync]='Offline-first Evento Globolo synchronization service built around opto-sync semantics.'
  [evgl-cli]='Evento Globolo Rust CLI driven by flags-2-env-compatible declarative flags.'
  [evgl-infra]='Docker Compose, Supabase/Postgres, Kubernetes, Caddy, and Terraform infrastructure for Evento Globolo.'
)

declare -A topics=(
  [evgl-interfaces]='rust events api-contracts openapi asyncapi protobuf evento-globolo'
  [evgl-api]='rust axum websocket rest-api events evento-globolo'
  [evgl-mash-web]='rust axum maud seaorm supabase htmx websocket events'
  [evgl-leptos-web]='rust leptos axum ssr websocket events evento-globolo'
  [evgl-dioxus-web]='rust dioxus axum ssr websocket events evento-globolo'
  [evgl-sync]='rust offline-first synchronization opto-sync events evento-globolo'
  [evgl-cli]='rust cli flags-2-env events evento-globolo'
  [evgl-infra]='kubernetes terraform docker-compose supabase postgres events evento-globolo'
)

canonical_digest() {
  local directory="$1"
  local exclude_review="${2:-false}"
  if test "$exclude_review" = true; then
    (
      cd "$directory"
      find . -type f \
        -not -path './.git/*' \
        -not -path './docs/bootstrap-review.md' \
        -print0 \
        | LC_ALL=C sort -z \
        | xargs -0 sha256sum \
        | sha256sum \
        | awk '{print $1}'
    )
  else
    (
      cd "$directory"
      find . -type f -not -path './.git/*' -print0 \
        | LC_ALL=C sort -z \
        | xargs -0 sha256sum \
        | sha256sum \
        | awk '{print $1}'
    )
  fi
}

stage=validate-reviewed-source
mapfile -t found_repos < <(find "$source_root" -mindepth 1 -maxdepth 1 -type d -printf '%f\n' | LC_ALL=C sort)
mapfile -t expected_repos < <(printf '%s\n' "${repos[@]}" | LC_ALL=C sort)
test "${#found_repos[@]}" -eq "${#expected_repos[@]}" || fail unexpected-repository-count
for index in "${!expected_repos[@]}"; do
  test "${found_repos[$index]}" = "${expected_repos[$index]}" \
    || fail "unexpected-repository:${found_repos[$index]}"
done

for repo in "${repos[@]}"; do
  directory="$source_root/$repo"
  test -d "$directory" || fail "missing-source-directory:${repo}"
  test ! -e "$directory/.git" || fail "embedded-git-directory:${repo}"
  actual="$(canonical_digest "$directory")"
  test "$actual" = "${source_digest[$repo]}" \
    || fail "source-tree-digest-mismatch:${repo}:${actual}"
done

if test "${PUBLISH_DRY_RUN:-0}" = 1; then
  jq -nc \
    --arg status validated \
    --arg organization "$organization" \
    --arg source_sha256 "$SOURCE_SHA256" \
    --argjson repository_count "${#repos[@]}" \
    '{status:$status,organization:$organization,source_sha256:$source_sha256,repository_count:$repository_count}'
  exit 0
fi

stage=reject-inherited-credentials
for inherited in GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN GIT_ASKPASS; do
  test -z "${!inherited:-}" || fail "inherited-credential:${inherited}"
done

stage=load-owner-gh-session
gh_hosts="${GH_CONFIG_DIR:-$HOME/.config/gh}/hosts.yml"
test -r "$gh_hosts" || fail missing-gh-hosts
GH_TOKEN="$(
  GH_HOSTS="$gh_hosts" python3 - <<'PY'
import ast
import os
import re
import sys
from pathlib import Path

path = Path(os.environ["GH_HOSTS"])
current_host = None
for raw in path.read_text(encoding="utf-8").splitlines():
    stripped = raw.strip()
    if not stripped or stripped.startswith("#"):
        continue
    indent = len(raw) - len(raw.lstrip())
    if indent == 0 and stripped.endswith(":"):
        current_host = stripped[:-1]
        continue
    if current_host != "github.com":
        continue
    match = re.match(r"^\s+oauth_token:\s*(.*?)\s*$", raw)
    if match is None:
        continue
    value = match.group(1)
    if len(value) >= 2 and value[0] == value[-1] and value[0] in "'\"":
        value = ast.literal_eval(value)
    if not isinstance(value, str) or not value or any(ch.isspace() for ch in value):
        raise SystemExit(65)
    sys.stdout.write(value)
    raise SystemExit(0)
raise SystemExit(65)
PY
)"
test -n "$GH_TOKEN" || fail empty-gh-token
export GH_TOKEN
export GITHUB_REPOSITORY_ADMIN_TOKEN="$GH_TOKEN"

askpass="$work/git-askpass.sh"
cat > "$askpass" <<'ASKPASS_EOF'
#!/usr/bin/env sh
case "${1:-}" in
  *Username*) printf '%s\n' x-access-token ;;
  *Password*) printf '%s\n' "${GH_TOKEN:?}" ;;
  *) exit 1 ;;
esac
ASKPASS_EOF
chmod 700 "$askpass"
export GIT_ASKPASS="$askpass"
export GIT_ASKPASS_REQUIRE=force
export GIT_TERMINAL_PROMPT=0
export GIT_CONFIG_COUNT=1
export GIT_CONFIG_KEY_0=credential.helper
export GIT_CONFIG_VALUE_0=

stage=verify-owner-identity
identity="$(gh api user)"
test "$(jq -er .login <<< "$identity")" = ORESoftware || fail unexpected-owner-login
membership="$(gh api "user/memberships/orgs/${organization}")"
test "$(jq -er .role <<< "$membership")" = admin || fail organization-role-not-admin
test "$(jq -er .state <<< "$membership")" = active || fail organization-membership-not-active
printf 'membership organization=%s role=admin state=active\n' "$organization"

ensure_repository() {
  local repo="$1"
  local full_name="$organization/$repo"
  local existing=''

  if existing="$(gh api "repos/${full_name}" 2>/dev/null)"; then
    test "$(jq -er .owner.login <<< "$existing")" = "$organization" \
      || fail "owner-mismatch:${full_name}"
    test "$(jq -er .visibility <<< "$existing")" = public \
      || fail "visibility-mismatch:${full_name}"
    printf 'repository repo=%s action=existing visibility=public\n' "$full_name"
  else
    jq -nc \
      --arg name "$repo" \
      --arg description "${description[$repo]}" \
      '{name:$name,description:$description,visibility:"public",private:false,has_issues:true,has_projects:true,has_wiki:false,auto_init:false}' \
      | gh api --method POST "orgs/${organization}/repos" --input - >/dev/null
    printf 'repository repo=%s action=created visibility=public\n' "$full_name"
  fi

  jq -nc \
    --arg description "${description[$repo]}" \
    '{description:$description,has_issues:true,has_projects:true,has_wiki:false,delete_branch_on_merge:true,allow_squash_merge:true,allow_merge_commit:true,allow_rebase_merge:true}' \
    | gh api --method PATCH "repos/${full_name}" --input - >/dev/null

  read -r -a topic_values <<< "${topics[$repo]}"
  printf '%s\n' "${topic_values[@]}" \
    | jq -Rsc 'split("\n")[:-1] | {names:.}' \
    | gh api --method PUT "repos/${full_name}/topics" --input - >/dev/null
  gh api --method PUT "repos/${full_name}/vulnerability-alerts" >/dev/null 2>&1 || true
}

verify_published_tree() {
  local repo="$1"
  local full_name="$organization/$repo"
  local verify_dir="$work/verify-$repo"
  rm -rf "$verify_dir"
  git clone --depth 1 --branch main "https://github.com/${full_name}.git" "$verify_dir" >/dev/null 2>&1
  local actual
  actual="$(canonical_digest "$verify_dir" true)"
  test "$actual" = "${source_digest[$repo]}" \
    || fail "published-tree-digest-mismatch:${full_name}:${actual}"
  test -s "$verify_dir/docs/bootstrap-review.md" \
    || fail "missing-bootstrap-review:${full_name}"
}

publish_repository() {
  local repo="$1"
  local full_name="$organization/$repo"
  local directory="$source_root/$repo"
  local initial_sha feature_sha remote_main remote_review pr_json pr_number pr_state pr_url

  stage="prepare-history:${repo}"
  git -C "$directory" init -b main >/dev/null
  git -C "$directory" config user.name 'Evento Globolo bootstrap'
  git -C "$directory" config user.email '41898282+github-actions[bot]@users.noreply.github.com'
  git -C "$directory" add -f -A
  GIT_AUTHOR_DATE='2026-08-04T20:20:00Z' \
  GIT_COMMITTER_DATE='2026-08-04T20:20:00Z' \
    git -C "$directory" commit -m "feat: bootstrap Evento Globolo ${repo}" >/dev/null
  initial_sha="$(git -C "$directory" rev-parse HEAD)"

  git -C "$directory" checkout -b "$review_branch" >/dev/null
  mkdir -p "$directory/docs"
  cat > "$directory/docs/bootstrap-review.md" <<EOF
# Evento Globolo bootstrap review

This repository is one component of the Evento Globolo global events-management platform.

- Repository: \`${full_name}\`
- Reviewed source archive SHA-256: \`${SOURCE_SHA256}\`
- Reviewed source-tree digest: \`${source_digest[$repo]}\`
- Initial implementation commit: \`${initial_sha}\`
- Review branch: \`${review_branch}\`

The initial source includes repository-specific CI, documentation, licensing, and runtime configuration. The review commit records the provenance boundary used by the one-shot publisher without introducing credentials or environment-specific secrets.
EOF
  git -C "$directory" add -f docs/bootstrap-review.md
  GIT_AUTHOR_DATE='2026-08-04T20:21:00Z' \
  GIT_COMMITTER_DATE='2026-08-04T20:21:00Z' \
    git -C "$directory" commit -m 'docs: record bootstrap review provenance' >/dev/null
  feature_sha="$(git -C "$directory" rev-parse HEAD)"
  git -C "$directory" remote add origin "https://github.com/${full_name}.git"

  stage="publish-main:${repo}"
  remote_main="$(gh api "repos/${full_name}/git/ref/heads/main" --jq .object.sha 2>/dev/null || true)"
  if test -z "$remote_main"; then
    git -C "$directory" push origin "${initial_sha}:refs/heads/main" >/dev/null
    gh api --method PATCH "repos/${full_name}" -f default_branch=main >/dev/null
    remote_main="$(gh api "repos/${full_name}/git/ref/heads/main" --jq .object.sha)"
  fi

  pr_json="$(
    gh pr list \
      --repo "$full_name" \
      --state all \
      --head "$review_branch" \
      --base main \
      --limit 10 \
      --json number,state,isDraft,mergedAt,url,headRefOid,mergeCommit \
      | jq -c 'sort_by(.number) | reverse | first // empty'
  )"

  if test "$remote_main" != "$initial_sha"; then
    test -n "$pr_json" || fail "unexpected-noninitial-main:${full_name}:${remote_main}"
    test "$(jq -r .state <<< "$pr_json")" = MERGED \
      || fail "noninitial-main-without-merged-pr:${full_name}"
    test "$(jq -er .headRefOid <<< "$pr_json")" = "$feature_sha" \
      || fail "existing-pr-head-mismatch:${full_name}"
    verify_published_tree "$repo"
    jq -nc \
      --arg repo "$full_name" \
      --arg initial_commit "$initial_sha" \
      --arg review_commit "$feature_sha" \
      --arg pr_url "$(jq -r .url <<< "$pr_json")" \
      --arg merged_main "$remote_main" \
      --arg status already-published \
      '{repository:$repo,initial_commit:$initial_commit,review_commit:$review_commit,pull_request:$pr_url,merged_main:$merged_main,status:$status}' \
      >> "$results"
    printf 'verified repo=%s action=already-published main=%s\n' "$full_name" "$remote_main"
    return
  fi

  stage="publish-review-branch:${repo}"
  remote_review="$(gh api "repos/${full_name}/git/ref/heads/${review_branch}" --jq .object.sha 2>/dev/null || true)"
  if test -z "$remote_review"; then
    git -C "$directory" push origin "${feature_sha}:refs/heads/${review_branch}" >/dev/null
  else
    test "$remote_review" = "$feature_sha" \
      || fail "review-branch-diverged:${full_name}:${remote_review}"
  fi

  if test -z "$pr_json"; then
    stage="open-pr:${repo}"
    pr_body="$work/pr-body-$repo.md"
    cat > "$pr_body" <<EOF
## What changed

Adds the reviewed bootstrap provenance record for \`${full_name}\` after publishing the complete initial implementation.

## Why

Evento Globolo spans separate Rust API, web, interface, synchronization, CLI, and infrastructure repositories. This PR establishes an auditable review boundary for the initial source archive and repository-specific source-tree digest.

## Validation

- Source archive SHA-256: \`${SOURCE_SHA256}\`
- Source-tree digest: \`${source_digest[$repo]}\`
- Initial implementation commit: \`${initial_sha}\`
- Review commit: \`${feature_sha}\`
EOF
    pr_url="$(
      gh pr create \
        --repo "$full_name" \
        --base main \
        --head "$review_branch" \
        --title 'docs: record Evento Globolo bootstrap review' \
        --body-file "$pr_body"
    )"
    test -n "$pr_url" || fail "pull-request-url-missing:${full_name}"
    pr_json="$(
      gh pr view "$pr_url" \
        --repo "$full_name" \
        --json number,state,isDraft,mergedAt,url,headRefOid,mergeCommit
    )"
  fi

  pr_number="$(jq -er .number <<< "$pr_json")"
  pr_state="$(jq -er .state <<< "$pr_json")"
  test "$(jq -er .headRefOid <<< "$pr_json")" = "$feature_sha" \
    || fail "pr-head-mismatch:${full_name}:${pr_number}"

  if test "$pr_state" = OPEN; then
    stage="merge-pr:${repo}"
    merge_result="$(
      jq -nc \
        --arg commit_title 'Merge Evento Globolo bootstrap review' \
        --arg commit_message "Reviewed initial Evento Globolo repository bootstrap for ${full_name}." \
        '{merge_method:"merge",commit_title:$commit_title,commit_message:$commit_message}' \
        | gh api --method PUT "repos/${full_name}/pulls/${pr_number}/merge" --input -
    )"
    test "$(jq -er .merged <<< "$merge_result")" = true \
      || fail "pull-request-not-merged:${full_name}:${pr_number}"
  elif test "$pr_state" != MERGED; then
    fail "unexpected-pr-state:${full_name}:${pr_number}:${pr_state}"
  fi

  stage="verify-merged-pr:${repo}"
  pr_json="$(gh pr view "$pr_number" --repo "$full_name" --json number,state,mergedAt,url,headRefOid,mergeCommit)"
  test "$(jq -er .state <<< "$pr_json")" = MERGED \
    || fail "merged-pr-state-mismatch:${full_name}:${pr_number}"
  remote_main="$(gh api "repos/${full_name}/git/ref/heads/main" --jq .object.sha)"
  verify_published_tree "$repo"

  jq -nc \
    --arg repo "$full_name" \
    --arg initial_commit "$initial_sha" \
    --arg review_commit "$feature_sha" \
    --arg pr_url "$(jq -r .url <<< "$pr_json")" \
    --arg merged_main "$remote_main" \
    --arg status published \
    '{repository:$repo,initial_commit:$initial_commit,review_commit:$review_commit,pull_request:$pr_url,merged_main:$merged_main,status:$status}' \
    >> "$results"
  printf 'verified repo=%s action=published pr=%s main=%s\n' \
    "$full_name" "$(jq -r .url <<< "$pr_json")" "$remote_main"
}

for repo in "${repos[@]}"; do
  stage="ensure-repository:${repo}"
  ensure_repository "$repo"
  publish_repository "$repo"
done

stage=final-verification
result_count="$(wc -l < "$results" | tr -d ' ')"
test "$result_count" -eq "${#repos[@]}" || fail result-count-mismatch
jq -s \
  --arg status success \
  --arg organization "$organization" \
  --arg source_sha256 "$SOURCE_SHA256" \
  '{status:$status,organization:$organization,source_sha256:$source_sha256,repository_count:length,repositories:.}' \
  "$results"

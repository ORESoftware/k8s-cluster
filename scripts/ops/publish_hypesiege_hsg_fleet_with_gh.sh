#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly ORG='hypesiege'
readonly EXPECTED_LOGIN='ORESoftware'
readonly TRACKING_REPOSITORY='ORESoftware/k8s-cluster'
readonly TRACKING_ISSUE='968'
readonly RELEASE_TAG='v0.1.0'
readonly RUN_KEY="${GITHUB_RUN_ID:-manual}-$(date -u +%Y%m%dT%H%M%SZ)"
readonly REPORT="${RUNNER_TEMP:-/tmp}/hypesiege-hsg-gh-publication.json"

work=''
cleanup() {
  local status=$?
  unset GH_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN
  if [[ -n "$work" && -d "$work" ]]; then
    rm -rf "$work"
  fi
  exit "$status"
}
trap cleanup EXIT INT TERM

fail() {
  printf 'hsg-gh-publisher status=failed stage=%s message=%s\n' "${stage:-unknown}" "$*" >&2
  exit 1
}

for command in gh git jq python3 sha256sum; do
  command -v "$command" >/dev/null || fail "required command unavailable: $command"
done

: "${GH_TOKEN:?GH_TOKEN is required from short-lived GitHub device OAuth}"
[[ -z "${GH_PAT:-}" ]] || fail 'GH_PAT must not be present'
[[ -z "${GITHUB_REPOSITORY_ADMIN_TOKEN:-}" ]] || fail 'repository-admin PAT must not be present'

stage='verify-owner'
[[ "$(gh api user --jq .login)" == "$EXPECTED_LOGIN" ]] || fail 'authenticated GitHub login is not ORESoftware'
membership="$(gh api "user/memberships/orgs/${ORG}")"
[[ "$(jq -r .state <<<"$membership")" == active ]] || fail 'hypesiege membership is not active'
[[ "$(jq -r .role <<<"$membership")" == admin ]] || fail 'hypesiege membership is not admin'
gh auth setup-git --hostname github.com --force >/dev/null

work="$(mktemp -d "${RUNNER_TEMP:-/tmp}/hsg-gh-publisher.XXXXXX")"
mkdir -p "$work/sources" "$work/targets"

readonly FEATURE_BRANCH="agent/canonical-hsg-initialization-${GITHUB_RUN_ID:-manual}"

declare -a targets=(
  hsg-api
  hsg-web-mash
  hsg-web-leptos
  hsg-web-dioxus
  hsg-infra
  hsg-cli
  hsg-interfaces
  hsg-sync
)

declare -A sources=(
  [hsg-api]='hypesiege-api-server.rs'
  [hsg-web-mash]='hypesiege-web-server.rs'
  [hsg-web-leptos]='hypesiege-monorepo'
  [hsg-web-dioxus]='hypesiege-monorepo'
  [hsg-infra]='hypesiege-infra'
  [hsg-cli]='hypesiege-cli'
  [hsg-interfaces]='hypesiege-interfaces'
  [hsg-sync]='hypesiege-sync'
)

declare -A source_shas=(
  [hsg-api]='bc9a0cad92afaad713c7dc919419eddf3c382668'
  [hsg-web-mash]='d46819cd52b10ec9b8115290f564df2b5072d0b1'
  [hsg-web-leptos]='9da72d0f584b1b61e0116891a1b36ebd2e0ff23a'
  [hsg-web-dioxus]='9da72d0f584b1b61e0116891a1b36ebd2e0ff23a'
  [hsg-infra]='def98e33918b8d56d09e7ea3d236f2a5ea57c6b9'
  [hsg-cli]='5851e7db947fac6ca6efbc0bdaee4aa8f03ed653'
  [hsg-interfaces]='3a1b30363f73e6f206fc42181750495b8af89dd4'
  [hsg-sync]='d7bf640f19f6bd69752363fd34b4d7396b309f0d'
)

declare -A prefixes=(
  [hsg-web-leptos]='starters/hsg-web-leptos'
  [hsg-web-dioxus]='starters/hsg-web-dioxus'
)

declare -A descriptions=(
  [hsg-api]='HypeSiege Rust API, publishing, approvals, engagement inbox, and WebSockets'
  [hsg-web-mash]='HypeSiege MASH server: Maud, Axum, SeaORM, Supabase Auth, htmx, and WebSockets'
  [hsg-web-leptos]='HypeSiege Leptos SSR web server and realtime dashboard'
  [hsg-web-dioxus]='HypeSiege Dioxus SSR web server and realtime dashboard'
  [hsg-infra]='HypeSiege Kubernetes, availability, and least-privilege infrastructure'
  [hsg-cli]='HypeSiege Rust CLI with flags-2-env'
  [hsg-interfaces]='HypeSiege SQL, OpenAPI, AsyncAPI, schemas, NATS, and generated contracts'
  [hsg-sync]='HypeSiege offline-first synchronization facade over opto-sync contracts'
)

stage='clone-reviewed-sources'
declare -A cloned
for target in "${targets[@]}"; do
  source="${sources[$target]}"
  if [[ -z "${cloned[$source]:-}" ]]; then
    path="$work/sources/$source"
    git init -q "$path"
    git -C "$path" remote add origin "https://github.com/${ORG}/${source}.git"
    git -C "$path" fetch -q origin "${source_shas[$target]}"
    [[ "$(git -C "$path" rev-parse FETCH_HEAD)" == "${source_shas[$target]}" ]] || fail "source SHA mismatch for $source"
    git -C "$path" fsck --full >/dev/null
    cloned[$source]=1
  fi
done

stage='prepare-reviewed-roots'
declare -A root_commits
for target in "${targets[@]}"; do
  source="${sources[$target]}"
  source_path="$work/sources/$source"
  source_sha="${source_shas[$target]}"
  prefix="${prefixes[$target]:-}"
  if [[ -n "$prefix" ]]; then
    branch="extract-${target}-${GITHUB_RUN_ID:-manual}"
    git -C "$source_path" branch -f reviewed-source "$source_sha" >/dev/null
    root_commits[$target]="$(git -C "$source_path" subtree split --prefix="$prefix" reviewed-source --branch "$branch")"
    git -C "$source_path" cat-file -e "${root_commits[$target]}^{commit}"
  else
    root_commits[$target]="$source_sha"
  fi
done

stage='ensure-private-repositories'
for target in "${targets[@]}"; do
  full="${ORG}/${target}"
  if ! gh repo view "$full" >/dev/null 2>&1; then
    gh repo create "$full" --private --description "${descriptions[$target]}" --disable-wiki >/dev/null
  fi
  metadata="$(gh api "repos/${full}")"
  [[ "$(jq -r .full_name <<<"$metadata")" == "$full" ]] || fail "repository identity mismatch: $full"
  [[ "$(jq -r .visibility <<<"$metadata")" == private ]] || fail "repository is not private: $full"
done

stage='prepare-and-push-initialization-branches'
results="$work/results.tsv"
: > "$results"
for target in "${targets[@]}"; do
  full="${ORG}/${target}"
  source="${sources[$target]}"
  source_sha="${source_shas[$target]}"
  source_root="${root_commits[$target]}"
  prefix="${prefixes[$target]:-}"
  target_path="$work/targets/$target"
  git init -q "$target_path"
  git -C "$target_path" config user.name 'ORESoftware HypeSiege publisher'
  git -C "$target_path" config user.email 'noreply@users.noreply.github.com'
  git -C "$target_path" remote add origin "https://github.com/${full}.git"
  git -C "$target_path" fetch -q "$work/sources/$source" "$source_root"
  source_root_local="$(git -C "$target_path" rev-parse FETCH_HEAD)"

  target_main=''
  if gh api "repos/${full}/git/ref/heads/main" > "$work/main-ref.json" 2>/dev/null; then
    target_main="$(jq -r .object.sha "$work/main-ref.json")"
    git -C "$target_path" fetch -q origin main
  fi

  git -C "$target_path" switch -q --detach "$source_root_local"
  cat > "$target_path/.hsg-repository.json" <<JSON
{
  "schema_version": 1,
  "target_repository": "${full}",
  "source_repository": "${ORG}/${source}",
  "source_ref": "main",
  "source_sha": "${source_sha}",
  "source_path": $(if [[ -n "$prefix" ]]; then jq -Rn --arg value "$prefix" '$value'; else printf 'null'; fi),
  "history_model": "$([[ -n "$prefix" ]] && printf 'git-subtree-split' || printf 'full-repository')",
  "publication_run": "${RUN_KEY}",
  "authenticated_login": "${EXPECTED_LOGIN}",
  "pat_used": false
}
JSON
  cat > "$target_path/MIGRATION.md" <<EOF
# Canonical HypeSiege repository

This repository was published from the reviewed HypeSiege source using a short-lived GitHub CLI device OAuth token authorized by \`${EXPECTED_LOGIN}\`.

- Target: \`${full}\`
- Source: \`${ORG}/${source}@${source_sha}\`
- Source path: \`${prefix:-repository root}\`
- History: \`$([[ -n "$prefix" ]] && printf 'git subtree split' || printf 'full repository history')\`
- Tracking: \`${TRACKING_REPOSITORY}#${TRACKING_ISSUE}\`
- PAT used: \`false\`

Future feature work should target this canonical \`hsg-*\` repository after downstream references are migrated.
EOF
  git -C "$target_path" add .hsg-repository.json MIGRATION.md
  git -C "$target_path" commit -q -m 'chore: establish canonical hsg repository identity'
  provenance_commit="$(git -C "$target_path" rev-parse HEAD)"

  if [[ -z "$target_main" ]]; then
    git -C "$target_path" push -q origin "$source_root_local:refs/heads/main"
    gh api --method PATCH "repos/${full}" -f default_branch=main -F delete_branch_on_merge=true >/dev/null
    target_main="$source_root_local"
    branch_commit="$provenance_commit"
  else
    provenance_tree="$(git -C "$target_path" rev-parse "${provenance_commit}^{tree}")"
    branch_commit="$(printf 'Import reviewed HypeSiege source and establish canonical hsg identity\n' | git -C "$target_path" commit-tree "$provenance_tree" -p "$target_main" -p "$provenance_commit")"
  fi

  branch="${FEATURE_BRANCH}"
  git -C "$target_path" push -q --force-with-lease="refs/heads/${branch}:" origin "$branch_commit:refs/heads/${branch}"

  pr_number="$(gh pr list --repo "$full" --state open --head "$branch" --json number --jq '.[0].number // empty')"
  if [[ -z "$pr_number" ]]; then
    pr_url="$(gh pr create --repo "$full" --base main --head "$branch" --title 'chore: initialize canonical hsg repository' --body "## What changed

- import the reviewed source at \`${source_sha}\`;
- preserve $([[ -n "$prefix" ]] && printf 'subtree' || printf 'full repository') history;
- record canonical \`${target}\` provenance;
- confirm short-lived GitHub CLI OAuth publication with no PAT.

Tracking: ${TRACKING_REPOSITORY}#${TRACKING_ISSUE}")"
    pr_number="${pr_url##*/}"
  fi
  printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$target" "$source" "$source_sha" "$branch_commit" "$pr_number" "$target_main" >> "$results"
done

wait_for_checks() {
  local full=$1 sha=$2
  local attempt checks statuses pending failed status_state checks_count statuses_count
  for attempt in $(seq 1 160); do
    checks="$(gh api "repos/${full}/commits/${sha}/check-runs?per_page=100")"
    statuses="$(gh api "repos/${full}/commits/${sha}/status")"
    pending="$(jq '[.check_runs[] | select(.status != "completed")] | length' <<<"$checks")"
    failed="$(jq '[.check_runs[] | select(.status == "completed" and ((.conclusion == "success" or .conclusion == "neutral" or .conclusion == "skipped") | not))] | length' <<<"$checks")"
    status_state="$(jq -r '.state // "pending"' <<<"$statuses")"
    (( failed == 0 )) || return 1
    [[ "$status_state" != failure && "$status_state" != error ]] || return 1
    checks_count="$(jq '.check_runs | length' <<<"$checks")"
    statuses_count="$(jq '.statuses | length' <<<"$statuses")"
    if (( pending == 0 )); then
      if (( checks_count > 0 )) || (( statuses_count > 0 )) || (( attempt >= 4 )); then
        [[ "$status_state" != pending ]] || (( statuses_count == 0 ))
        return 0
      fi
    fi
    sleep 10
  done
  return 1
}

stage='validate-and-merge-pull-requests'
merged="$work/merged.tsv"
: > "$merged"
while IFS=$'\t' read -r target source source_sha branch_commit pr_number target_main; do
  full="${ORG}/${target}"
  wait_for_checks "$full" "$branch_commit" || fail "checks failed or timed out for ${full}#${pr_number}"
  gh pr ready "$pr_number" --repo "$full" >/dev/null 2>&1 || true
  gh pr merge "$pr_number" --repo "$full" --merge --admin --delete-branch >/dev/null
  pr_state="$(gh pr view "$pr_number" --repo "$full" --json state --jq .state)"
  [[ "$pr_state" == MERGED ]] || fail "pull request did not merge: ${full}#${pr_number}"
  main_sha="$(gh api "repos/${full}/git/ref/heads/main" --jq .object.sha)"
  printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$target" "$source" "$source_sha" "$branch_commit" "$pr_number" "$main_sha" >> "$merged"
done < "$results"

stage='tag-and-release'
while IFS=$'\t' read -r target source source_sha branch_commit pr_number main_sha; do
  full="${ORG}/${target}"
  if ! gh api "repos/${full}/git/ref/tags/${RELEASE_TAG}" >/dev/null 2>&1; then
    gh api --method POST "repos/${full}/git/refs" -f ref="refs/tags/${RELEASE_TAG}" -f sha="$main_sha" >/dev/null
  fi
  if ! gh release view "$RELEASE_TAG" --repo "$full" >/dev/null 2>&1; then
    gh release create "$RELEASE_TAG" --repo "$full" --title "${target} ${RELEASE_TAG}" --notes "Initial canonical HypeSiege repository publication from ${ORG}/${source}@${source_sha}. PAT used: false." >/dev/null
  fi
done < "$merged"

stage='verify-remotes'
: > "$work/repositories.jsonl"
while IFS=$'\t' read -r target source source_sha branch_commit pr_number main_sha; do
  full="${ORG}/${target}"
  metadata="$(gh api "repos/${full}")"
  [[ "$(jq -r .visibility <<<"$metadata")" == private ]] || fail "visibility mismatch for $full"
  [[ "$(jq -r .default_branch <<<"$metadata")" == main ]] || fail "default branch mismatch for $full"
  marker="$(gh api "repos/${full}/contents/.hsg-repository.json?ref=main" --jq .content | tr -d '\n' | base64 --decode)"
  [[ "$(jq -r .source_sha <<<"$marker")" == "$source_sha" ]] || fail "source marker mismatch for $full"
  [[ "$(jq -r .pat_used <<<"$marker")" == false ]] || fail "PAT marker mismatch for $full"
  [[ "$(gh pr view "$pr_number" --repo "$full" --json state --jq .state)" == MERGED ]] || fail "PR verification failed for $full"
  [[ "$(gh api "repos/${full}/git/ref/tags/${RELEASE_TAG}" --jq .ref)" == "refs/tags/${RELEASE_TAG}" ]] || fail "tag missing for $full"
  jq -n --arg repository "$full" --arg source "${ORG}/${source}" --arg source_sha "$source_sha" --arg main_sha "$main_sha" --argjson pull_request "$pr_number" '{repository:$repository,source:$source,source_sha:$source_sha,main_sha:$main_sha,pull_request:$pull_request,pull_request_state:"MERGED",visibility:"private",default_branch:"main",tag:"v0.1.0",pat_used:false}' >> "$work/repositories.jsonl"
done < "$merged"

jq -s --arg organization "$ORG" --arg login "$EXPECTED_LOGIN" --arg run_key "$RUN_KEY" '{schema_version:1,organization:$organization,authenticated_login:$login,run_key:$run_key,pat_used:false,repository_count:length,merged_pull_request_count:([.[]|select(.pull_request_state=="MERGED")]|length),repositories:.}' "$work/repositories.jsonl" > "$REPORT"
jq -e '.pat_used == false and .repository_count == 8 and .merged_pull_request_count == 8 and all(.repositories[]; .visibility == "private" and .default_branch == "main" and .pull_request_state == "MERGED" and .tag == "v0.1.0")' "$REPORT" >/dev/null

printf 'HYPESIEGE_HSG_GH_PUBLICATION_COMPLETE repositories=8 merged_prs=8 report=%s\n' "$REPORT"

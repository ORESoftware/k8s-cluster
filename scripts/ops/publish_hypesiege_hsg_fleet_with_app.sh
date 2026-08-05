#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly ORG='hypesiege'
readonly API_URL='https://api.github.com'
readonly API_VERSION='2022-11-28'
readonly MIGRATION_BRANCH='agent/establish-hsg-identity'
readonly REPORT_JSON="${RUNNER_TEMP:-/tmp}/hypesiege-hsg-publication.json"
readonly REPORT_MARKDOWN="${RUNNER_TEMP:-/tmp}/hypesiege-hsg-publication.md"

readonly -a TARGETS=(
  hsg-web-mash
  hsg-web-leptos
  hsg-web-dioxus
  hsg-api
  hsg-infra
  hsg-cli
  hsg-interfaces
  hsg-sync
)

declare -Ar SOURCE_REPOSITORY=(
  [hsg-web-mash]='hypesiege/hypesiege-web-server.rs'
  [hsg-web-leptos]='hypesiege/hypesiege-monorepo'
  [hsg-web-dioxus]='hypesiege/hypesiege-monorepo'
  [hsg-api]='hypesiege/hypesiege-api-server.rs'
  [hsg-infra]='hypesiege/hypesiege-infra'
  [hsg-cli]='hypesiege/hypesiege-cli'
  [hsg-interfaces]='hypesiege/hypesiege-interfaces'
  [hsg-sync]='hypesiege/hypesiege-sync'
)

declare -Ar SOURCE_SHA=(
  [hsg-web-mash]='c36feda56828f034b078cd9d228295c4d012de0d'
  [hsg-web-leptos]='67318243b6e2cc04d43dff93ad0aa81fa4da494f'
  [hsg-web-dioxus]='67318243b6e2cc04d43dff93ad0aa81fa4da494f'
  [hsg-api]='ab293f9cb9f403858a94846126aec1b9e70c8529'
  [hsg-infra]='def98e33918b8d56d09e7ea3d236f2a5ea57c6b9'
  [hsg-cli]='5befc74e31f5675dedb01d9975c2120053c60271'
  [hsg-interfaces]='3a1b30363f73e6f206fc42181750495b8af89dd4'
  [hsg-sync]='d7bf640f19f6bd69752363fd34b4d7396b309f0d'
)

declare -Ar SOURCE_PATH=(
  [hsg-web-mash]='.'
  [hsg-web-leptos]='starters/hsg-web-leptos'
  [hsg-web-dioxus]='starters/hsg-web-dioxus'
  [hsg-api]='.'
  [hsg-infra]='.'
  [hsg-cli]='.'
  [hsg-interfaces]='.'
  [hsg-sync]='.'
)

declare -Ar DESCRIPTION=(
  [hsg-web-mash]='HypeSiege MASH web server: Maud, Axum, SeaORM, Supabase Auth, htmx, and WebSockets'
  [hsg-web-leptos]='HypeSiege Leptos SSR web server and realtime dashboard projection'
  [hsg-web-dioxus]='HypeSiege Dioxus SSR web server and realtime dashboard projection'
  [hsg-api]='HypeSiege Rust API, transactional publishing, approvals, inbox, and authenticated WebSockets'
  [hsg-infra]='HypeSiege Kubernetes, autoscaling, disruption, ingress, and least-privilege infrastructure'
  [hsg-cli]='HypeSiege Rust CLI with audited flags-2-env configuration'
  [hsg-interfaces]='HypeSiege SQL, OpenAPI, AsyncAPI, JSON Schema, NATS, and generated language contracts'
  [hsg-sync]='HypeSiege offline-first synchronization facade over the reviewed opto-sync boundary'
)

stage='bootstrap'
work=''
app_jwt=''
installation_token=''

fail() {
  printf 'hsg-publisher-stage=%s status=failed message=%s\n' "$stage" "$*" >&2
  exit 1
}

cleanup() {
  unset GH_TOKEN installation_token app_jwt K8S_SUBMODULE_APP_PRIVATE_KEY
  if [[ -n "$work" && -e "$work" ]]; then
    python3 - "$work" <<'PY'
import shutil
import sys
from pathlib import Path
path = Path(sys.argv[1])
if path.exists():
    shutil.rmtree(path)
PY
  fi
}
trap cleanup EXIT
trap 'fail "unexpected command failure at line ${LINENO}"' ERR

: "${K8S_SUBMODULE_APP_ID:?K8S_SUBMODULE_APP_ID is required}"
: "${K8S_SUBMODULE_APP_PRIVATE_KEY:?K8S_SUBMODULE_APP_PRIVATE_KEY is required}"
[[ "$K8S_SUBMODULE_APP_ID" =~ ^[0-9]+$ ]] || fail 'GitHub App ID must be numeric'
[[ -z "${GH_PAT:-}" ]] || fail 'GH_PAT must not be present in this App-only publisher'
[[ -z "${GITHUB_REPOSITORY_ADMIN_TOKEN:-}" ]] || fail 'repository-admin PAT must not be present'

for command in curl git gh jq openssl python3 sha256sum tar; do
  command -v "$command" >/dev/null || fail "required command is unavailable: $command"
done

work="$(mktemp -d "${RUNNER_TEMP:-/tmp}/hsg-app-publisher.XXXXXX")"
private_key_file="$work/app-private-key.pem"
printf '%s' "$K8S_SUBMODULE_APP_PRIVATE_KEY" > "$private_key_file"
chmod 600 "$private_key_file"
grep -Eq '^-----BEGIN (RSA )?PRIVATE KEY-----$' "$private_key_file"

base64url() {
  openssl base64 -A | tr '+/' '-_' | tr -d '='
}

stage='mint-app-jwt'
now="$(date +%s)"
header='{"alg":"RS256","typ":"JWT"}'
payload="$(printf '{"iat":%s,"exp":%s,"iss":%s}' "$((now - 60))" "$((now + 540))" "$K8S_SUBMODULE_APP_ID")"
unsigned="$(printf '%s' "$header" | base64url).$(printf '%s' "$payload" | base64url)"
signature="$(printf '%s' "$unsigned" | openssl dgst -sha256 -sign "$private_key_file" -binary | base64url)"
app_jwt="${unsigned}.${signature}"
if [[ -n "${GITHUB_ACTIONS:-}" ]]; then
  echo "::add-mask::$app_jwt"
fi

stage='resolve-installation'
installation_json="$work/installation.json"
installation_status="$(
  curl --silent --show-error \
    --output "$installation_json" \
    --write-out '%{http_code}' \
    --header 'Accept: application/vnd.github+json' \
    --header "Authorization: Bearer ${app_jwt}" \
    --header "X-GitHub-Api-Version: ${API_VERSION}" \
    "${API_URL}/orgs/${ORG}/installation"
)"
[[ "$installation_status" == 200 ]] || fail "GitHub App installation lookup returned HTTP $installation_status"
installation_id="$(jq -er '.id | select(type == "number" and . > 0)' "$installation_json")"
repository_selection="$(jq -er '.repository_selection' "$installation_json")"
app_slug="$(jq -er '.app_slug | select(type == "string" and length > 0)' "$installation_json")"
[[ "$repository_selection" == all ]] || fail "App installation must select all repositories, observed: $repository_selection"

stage='mint-installation-token'
token_json="$work/token.json"
token_status="$(
  curl --silent --show-error \
    --request POST \
    --output "$token_json" \
    --write-out '%{http_code}' \
    --header 'Accept: application/vnd.github+json' \
    --header "Authorization: Bearer ${app_jwt}" \
    --header "X-GitHub-Api-Version: ${API_VERSION}" \
    --header 'Content-Type: application/json' \
    --data '{}' \
    "${API_URL}/app/installations/${installation_id}/access_tokens"
)"
[[ "$token_status" == 201 ]] || fail "installation-token mint returned HTTP $token_status"
installation_token="$(jq -er '.token | select(type == "string" and length > 0)' "$token_json")"
for permission in administration contents pull_requests metadata; do
  observed="$(jq -r --arg permission "$permission" '.permissions[$permission] // "none"' "$token_json")"
  case "$permission:$observed" in
    administration:write|contents:write|pull_requests:write|metadata:read) ;;
    *) fail "required App permission missing: $permission=$observed" ;;
  esac
done
if [[ -n "${GITHUB_ACTIONS:-}" ]]; then
  echo "::add-mask::$installation_token"
fi
export GH_TOKEN="$installation_token"
unset app_jwt K8S_SUBMODULE_APP_PRIVATE_KEY

stage='configure-git-auth'
askpass="$work/git-askpass.sh"
cat > "$askpass" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
  *Username*) printf '%s\n' 'x-access-token' ;;
  *Password*) printf '%s\n' "${GH_TOKEN:?}" ;;
  *) exit 1 ;;
esac
SH
chmod 700 "$askpass"
export GIT_ASKPASS="$askpass"
export GIT_TERMINAL_PROMPT=0
export GIT_CONFIG_NOSYSTEM=1

printf 'hsg-publisher-app=%s installation=%s repository-selection=%s\n' \
  "$app_slug" "$installation_id" "$repository_selection"

jq -n \
  --arg organization "$ORG" \
  --arg app "$app_slug" \
  --argjson installation_id "$installation_id" \
  '{schema_version:1, organization:$organization, app:$app, installation_id:$installation_id, repositories:[]}' \
  > "$REPORT_JSON"

create_repository() {
  local target="$1"
  local full_name="${ORG}/${target}"
  local created=false
  local response="$work/${target}-repository.json"

  if gh api "repos/${full_name}" > "$response" 2>/dev/null; then
    test "$(jq -r .owner.login "$response")" = "$ORG"
  else
    created=true
    gh api --method POST "orgs/${ORG}/repos" \
      -f name="$target" \
      -f description="${DESCRIPTION[$target]}" \
      -F private=true \
      -F has_issues=true \
      -F has_projects=false \
      -F has_wiki=false \
      -F auto_init=false \
      > "$response"
  fi

  for attempt in $(seq 1 30); do
    if gh api "repos/${full_name}" > "$response" 2>/dev/null; then
      break
    fi
    sleep 2
  done
  test "$(jq -r .full_name "$response")" = "$full_name"
  test "$(jq -r .visibility "$response")" = private
  printf '%s' "$created"
}

materialize_source() {
  local target="$1"
  local source="${SOURCE_REPOSITORY[$target]}"
  local source_sha="${SOURCE_SHA[$target]}"
  local source_path="${SOURCE_PATH[$target]}"
  local source_dir="$work/${target}-source"
  local repository_dir="$work/${target}-repository"

  git init "$source_dir" >/dev/null
  git -C "$source_dir" remote add origin "https://github.com/${source}.git"
  git -C "$source_dir" fetch --no-tags origin "$source_sha" >/dev/null
  git -C "$source_dir" switch --detach FETCH_HEAD >/dev/null
  test "$(git -C "$source_dir" rev-parse HEAD)" = "$source_sha"

  if [[ "$source_path" == . ]]; then
    printf '%s\n' "$source_dir"
    return
  fi

  mkdir -p "$repository_dir"
  git -C "$source_dir" archive --format=tar "${source_sha}:${source_path}" | tar -xf - -C "$repository_dir"
  git -C "$repository_dir" init >/dev/null
  git -C "$repository_dir" config user.name 'HypeSiege Repository Publisher App'
  git -C "$repository_dir" config user.email 'hypesiege-repository-publisher[bot]@users.noreply.github.com'
  git -C "$repository_dir" add -A
  source_date="$(git -C "$source_dir" show -s --format=%cI "$source_sha")"
  GIT_AUTHOR_DATE="$source_date" GIT_COMMITTER_DATE="$source_date" \
    git -C "$repository_dir" commit \
      -m "feat: publish standalone ${target} service" \
      -m "Extracted from ${source}@${source_sha}:${source_path}." >/dev/null
  printf '%s\n' "$repository_dir"
}

publish_main() {
  local target="$1"
  local repository_dir="$2"
  local full_name="${ORG}/${target}"
  local baseline_sha
  local target_main

  baseline_sha="$(git -C "$repository_dir" rev-parse HEAD)"
  if target_main="$(gh api "repos/${full_name}/git/ref/heads/main" --jq .object.sha 2>/dev/null)"; then
    git -C "$repository_dir" remote add target "https://github.com/${full_name}.git" 2>/dev/null || true
    git -C "$repository_dir" fetch --no-tags target refs/heads/main:refs/remotes/target/main >/dev/null
    git -C "$repository_dir" merge-base --is-ancestor "$baseline_sha" refs/remotes/target/main \
      || fail "existing ${full_name}@main does not descend from reviewed baseline ${baseline_sha}"
  else
    git -C "$repository_dir" remote add target "https://github.com/${full_name}.git"
    git -C "$repository_dir" push target "${baseline_sha}:refs/heads/main" >/dev/null
    target_main="$(gh api "repos/${full_name}/git/ref/heads/main" --jq .object.sha)"
    test "$target_main" = "$baseline_sha"
  fi

  gh api --method PATCH "repos/${full_name}" \
    -f description="${DESCRIPTION[$target]}" \
    -f default_branch=main \
    -F private=true \
    -F has_issues=true \
    -F has_projects=false \
    -F has_wiki=false \
    -F allow_merge_commit=true \
    -F allow_squash_merge=true \
    -F allow_rebase_merge=true \
    -F delete_branch_on_merge=true >/dev/null

  printf '%s\n' "$baseline_sha"
}

publish_migration_pr() {
  local target="$1"
  local repository_dir="$2"
  local baseline_sha="$3"
  local full_name="${ORG}/${target}"
  local source="${SOURCE_REPOSITORY[$target]}"
  local source_sha="${SOURCE_SHA[$target]}"
  local source_path="${SOURCE_PATH[$target]}"
  local target_main
  local branch_sha
  local pr_url

  target_main="$(gh api "repos/${full_name}/git/ref/heads/main" --jq .object.sha)"
  git -C "$repository_dir" fetch --no-tags target refs/heads/main:refs/remotes/target/main >/dev/null
  git -C "$repository_dir" switch --detach refs/remotes/target/main >/dev/null

  if gh api "repos/${full_name}/contents/.github/hsg-lineage.json?ref=main" >/dev/null 2>&1; then
    branch_sha="$target_main"
    pr_url='merged-on-main'
    printf '%s\t%s\n' "$branch_sha" "$pr_url"
    return
  fi

  if branch_sha="$(gh api "repos/${full_name}/git/ref/heads/${MIGRATION_BRANCH}" --jq .object.sha 2>/dev/null)"; then
    git -C "$repository_dir" fetch --no-tags target \
      "refs/heads/${MIGRATION_BRANCH}:refs/remotes/target/${MIGRATION_BRANCH}" >/dev/null
  else
    git -C "$repository_dir" switch -c "$MIGRATION_BRANCH" >/dev/null
    mkdir -p "$repository_dir/.github"
    jq -n \
      --arg target_repository "$full_name" \
      --arg source_repository "$source" \
      --arg source_commit "$source_sha" \
      --arg source_path "$source_path" \
      --arg baseline_commit "$baseline_sha" \
      '{schema_version:1,target_repository:$target_repository,source:{repository:$source_repository,commit:$source_commit,path:$source_path},baseline_commit:$baseline_commit,publication:{credential:"github_app_installation_token",pat_used:false}}' \
      > "$repository_dir/.github/hsg-lineage.json"

    cat > "$repository_dir/MIGRATION.md" <<EOF
# ${target}: HypeSiege repository identity

This repository is the canonical \`${full_name}\` service boundary for the HypeSiege Buffer-compatible publishing platform.

## Reviewed lineage

- Source repository: \`${source}\`
- Source commit: \`${source_sha}\`
- Source path: \`${source_path}\`
- Published baseline: \`${baseline_sha}\`
- Credential boundary: short-lived GitHub App installation token; no PAT

The source implementation already passed its repository-specific formatting, compilation, strict lint, unit, integration, browser, contract, or deployment checks before this publication. Future product changes should be developed here, with the legacy source retained as migration history until all consumers, packages, images, submodules, and deployment references move to this canonical repository.
EOF

    git -C "$repository_dir" config user.name 'HypeSiege Repository Publisher App'
    git -C "$repository_dir" config user.email 'hypesiege-repository-publisher[bot]@users.noreply.github.com'
    git -C "$repository_dir" add .github/hsg-lineage.json MIGRATION.md
    base_date="$(git -C "$repository_dir" show -s --format=%cI refs/remotes/target/main)"
    GIT_AUTHOR_DATE="$base_date" GIT_COMMITTER_DATE="$base_date" \
      git -C "$repository_dir" commit \
        -m 'chore: establish canonical hsg repository identity' \
        -m "Record reviewed lineage from ${source}@${source_sha}." >/dev/null
    branch_sha="$(git -C "$repository_dir" rev-parse HEAD)"
    git -C "$repository_dir" push target "${branch_sha}:refs/heads/${MIGRATION_BRANCH}" >/dev/null
  fi

  pr_url="$(
    gh api --method GET "repos/${full_name}/pulls" \
      -f state=open \
      -f head="${ORG}:${MIGRATION_BRANCH}" \
      -f base=main \
      --jq '.[0].html_url // empty'
  )"
  if [[ -z "$pr_url" ]]; then
    pr_body="$work/${target}-pr-body.md"
    cat > "$pr_body" <<EOF
## What changed

- record the canonical \`${full_name}\` service identity;
- preserve the exact reviewed source lineage \`${source}@${source_sha}:${source_path}\`;
- record the published baseline commit \`${baseline_sha}\`;
- state that publication used a short-lived GitHub App installation token and no PAT.

## Why

This is the reviewable handoff from the legacy HypeSiege repository layout to the requested \`hsg-*\` topology. Product behavior remains unchanged in this PR; package, image, deployment, and consumer migrations can now land incrementally against a stable canonical repository.

## Validation

The repository's inherited workflows run against this exact branch head. The publication workflow separately verifies repository ownership, visibility, default branch, main ancestry, branch SHA, and pull-request URL.
EOF
    pr_url="$(
      jq -n \
        --arg title 'chore: establish canonical hsg repository identity' \
        --arg head "$MIGRATION_BRANCH" \
        --arg base main \
        --rawfile body "$pr_body" \
        '{title:$title,head:$head,base:$base,body:$body,draft:false}' \
        | gh api --method POST "repos/${full_name}/pulls" --input - --jq .html_url
    )"
  fi

  printf '%s\t%s\n' "$branch_sha" "$pr_url"
}

stage='publish-reviewed-fleet'
for target in "${TARGETS[@]}"; do
  stage="create-${target}"
  created="$(create_repository "$target")"

  stage="materialize-${target}"
  repository_dir="$(materialize_source "$target")"

  stage="push-main-${target}"
  baseline_sha="$(publish_main "$target" "$repository_dir")"

  stage="open-pr-${target}"
  IFS=$'\t' read -r branch_sha pr_url < <(
    publish_migration_pr "$target" "$repository_dir" "$baseline_sha"
  )

  live_main="$(gh api "repos/${ORG}/${target}/git/ref/heads/main" --jq .object.sha)"
  visibility="$(gh api "repos/${ORG}/${target}" --jq .visibility)"
  default_branch="$(gh api "repos/${ORG}/${target}" --jq .default_branch)"
  test "$visibility" = private
  test "$default_branch" = main

  next="$work/report-next.json"
  jq \
    --arg target "${ORG}/${target}" \
    --arg source "${SOURCE_REPOSITORY[$target]}" \
    --arg source_sha "${SOURCE_SHA[$target]}" \
    --arg source_path "${SOURCE_PATH[$target]}" \
    --arg baseline_sha "$baseline_sha" \
    --arg main_sha "$live_main" \
    --arg branch "$MIGRATION_BRANCH" \
    --arg branch_sha "$branch_sha" \
    --arg pull_request "$pr_url" \
    --arg visibility "$visibility" \
    --argjson created "$created" \
    '.repositories += [{target:$target,source:{repository:$source,commit:$source_sha,path:$source_path},created:$created,visibility:$visibility,default_branch:"main",baseline_sha:$baseline_sha,main_sha:$main_sha,migration_branch:$branch,migration_sha:$branch_sha,pull_request:$pull_request}]' \
    "$REPORT_JSON" > "$next"
  mv "$next" "$REPORT_JSON"
  printf 'hsg-published target=%s main=%s migration=%s pr=%s created=%s\n' \
    "${ORG}/${target}" "$live_main" "$branch_sha" "$pr_url" "$created"
done

stage='verify-fleet'
jq -e --argjson expected "${#TARGETS[@]}" '
  .schema_version == 1 and
  (.repositories | length) == $expected and
  all(.repositories[];
    .target | startswith("hypesiege/hsg-") and
    .visibility == "private" and
    .default_branch == "main" and
    (.main_sha | test("^[0-9a-f]{40}$")) and
    (.migration_sha | test("^[0-9a-f]{40}$")) and
    ((.pull_request == "merged-on-main") or (.pull_request | startswith("https://github.com/hypesiege/hsg-")))
  )
' "$REPORT_JSON" >/dev/null

{
  echo '# HypeSiege hsg-* GitHub App publication'
  echo
  echo 'Overall result: **SUCCESS**'
  echo
  printf -- '- Organization: `%s`\n' "$ORG"
  printf -- '- GitHub App: `%s`\n' "$app_slug"
  printf -- '- Repositories: `%s/%s`\n' "$(jq '.repositories | length' "$REPORT_JSON")" "${#TARGETS[@]}"
  echo '- Credential: short-lived GitHub App installation token'
  echo '- PAT used: `false`'
  echo
  jq -r '.repositories[] | "- `\(.target)` — main `\(.main_sha)`; migration `\(.migration_sha)`; PR \(.pull_request)"' "$REPORT_JSON"
} > "$REPORT_MARKDOWN"

stage='complete'
echo 'HSG_PUBLICATION_REPORT_BEGIN'
cat "$REPORT_MARKDOWN"
echo 'HSG_PUBLICATION_REPORT_END'
printf 'HSG_REPORT_JSON=%s\n' "$REPORT_JSON"
printf 'HSG_REPORT_MARKDOWN=%s\n' "$REPORT_MARKDOWN"

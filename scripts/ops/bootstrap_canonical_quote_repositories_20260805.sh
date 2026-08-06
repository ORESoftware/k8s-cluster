#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly ORG='canonical-cloud'
readonly -a REPOSITORIES=(
  'canonical-api-server.rs'
  'canonical-infra'
  'canonical-lib'
  'canonical-flutter'
)
declare -Ar DESCRIPTIONS=(
  ['canonical-api-server.rs']='Canonical quote REST and WebSocket API in Rust with Axum, SeaORM, Shared Auth, and Gemini analysis.'
  ['canonical-infra']='Canonical Cloudflare, Kubernetes, Postgres, and deployment infrastructure.'
  ['canonical-lib']='Shared Canonical Rust domain types and quote intake validation.'
  ['canonical-flutter']='Canonical Flutter companion application for authenticated quote workflows.'
)

validate_scope() {
  [[ ${#REPOSITORIES[@]} -eq 4 ]]
  local -A seen=()
  local repository
  for repository in "${REPOSITORIES[@]}"; do
    [[ $repository =~ ^canonical-[A-Za-z0-9._-]+$ ]]
    [[ -n ${DESCRIPTIONS[$repository]:-} ]]
    [[ -z ${seen[$repository]:-} ]]
    seen[$repository]=1
  done
}

validate_scope
if [[ ${1:-} == '--validate-only' ]]; then
  printf 'VERIFIED bounded Canonical quote repository bootstrap (%d repositories)\n' "${#REPOSITORIES[@]}"
  exit 0
fi

readonly TRUSTED_SHA="${1:?trusted source SHA is required}"
[[ $TRUSTED_SHA =~ ^[0-9a-f]{40}$ ]]

for inherited in GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN GIT_ASKPASS; do
  if [[ -n ${!inherited:-} ]]; then
    printf 'refusing inherited credential variable: %s\n' "$inherited" >&2
    exit 64
  fi
done

publisher="$(gh api user --jq .login)"
[[ $publisher == 'ORESoftware' ]]
membership="$(gh api "user/memberships/orgs/$ORG" --jq '.role + ":" + .state')"
[[ $membership == 'admin:active' ]]
printf 'VERIFIED protected publisher=%s org=%s membership=%s source=%s\n' \
  "$publisher" "$ORG" "$membership" "$TRUSTED_SHA"

work="$(mktemp -d /tmp/canonical-quote-repository-bootstrap.XXXXXX)"
cleanup() { rm -rf "$work"; }
trap cleanup EXIT

for repository in "${REPOSITORIES[@]}"; do
  full_name="$ORG/$repository"
  metadata="$work/${repository//\//_}.json"
  disposition='PRESERVED'

  if ! gh api "repos/$full_name" >"$metadata" 2>/dev/null; then
    gh api --method POST "orgs/$ORG/repos" \
      -f name="$repository" \
      -f description="${DESCRIPTIONS[$repository]}" \
      -F private=true \
      -F has_issues=true \
      -F has_projects=true \
      -F has_wiki=false \
      -F auto_init=true >"$metadata"
    disposition='CREATED'
  fi

  actual_full_name="$(jq -r .full_name "$metadata")"
  is_private="$(jq -r .private "$metadata")"
  [[ ${actual_full_name,,} == ${full_name,,} ]]
  [[ $is_private == 'true' ]]

  default_branch="$(jq -r '.default_branch // empty' "$metadata")"
  [[ -n $default_branch ]]
  if [[ $default_branch != 'main' ]]; then
    encoded_branch="$(jq -rn --arg value "$default_branch" '$value|@uri')"
    gh api --method POST "repos/$full_name/branches/$encoded_branch/rename" \
      -f new_name='main' >/dev/null
  fi

  gh api --method PATCH "repos/$full_name" \
    -f description="${DESCRIPTIONS[$repository]}" \
    -f default_branch='main' \
    -F private=true \
    -F has_issues=true \
    -F has_projects=true \
    -F has_wiki=false \
    -F delete_branch_on_merge=true >"$metadata"

  [[ $(jq -r .full_name "$metadata") == "$full_name" ]]
  [[ $(jq -r .private "$metadata") == 'true' ]]
  [[ $(jq -r .default_branch "$metadata") == 'main' ]]
  printf 'VERIFIED_%s_PRIVATE %s default_branch=main\n' "$disposition" "$full_name"
done

printf 'VERIFIED Canonical quote repository bootstrap total=4 overwrite=0\n'

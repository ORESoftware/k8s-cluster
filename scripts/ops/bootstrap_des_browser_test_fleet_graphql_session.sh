#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

: "${ISSUE_TOKEN:?ISSUE_TOKEN is required}"
: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
: "${GITHUB_RUN_ID:?GITHUB_RUN_ID is required}"
: "${TRACKING_ISSUE:?TRACKING_ISSUE is required}"

readonly TARGET_ORG='discrete-event-systems-test'
readonly EXPECTED_LOGIN='ORESoftware'
readonly PROJECT_TITLE='DES Browser Automation'
readonly API_VERSION='2022-11-28'
readonly HANDOFF_TIMEOUT_SECONDS=840

work="$(mktemp -d "${RUNNER_TEMP:-/tmp}/des-browser-graphql-session.XXXXXX")"
private_key="$work/session-private.pem"
public_der="$work/session-public.der"
request_comment_id=''
envelope_comment_id=''
github_token=''

issue_api() {
  local method="$1" endpoint="$2" data_file="${3:-}"
  local args=(
    --silent --show-error --fail-with-body
    --request "$method"
    --header 'Accept: application/vnd.github+json'
    --header "Authorization: Bearer ${ISSUE_TOKEN}"
    --header "X-GitHub-Api-Version: ${API_VERSION}"
  )
  if [[ -n "$data_file" ]]; then
    args+=(--header 'Content-Type: application/json' --data-binary "@$data_file")
  fi
  curl "${args[@]}" "https://api.github.com/repos/${GITHUB_REPOSITORY}${endpoint}"
}

delete_comment() {
  local comment_id="${1:-}"
  [[ -n "$comment_id" ]] || return 0
  issue_api DELETE "/issues/comments/${comment_id}" >/dev/null 2>&1 || true
}

cleanup() {
  local status=$?
  delete_comment "$envelope_comment_id"
  delete_comment "$request_comment_id"
  unset ISSUE_TOKEN GH_TOKEN GITHUB_TOKEN github_token
  find "$work" -type f -exec sh -c 'for file do : > "$file"; done' sh {} + 2>/dev/null || true
  rm -rf "$work"
  exit "$status"
}
trap cleanup EXIT INT TERM

for tool in base64 curl jq openssl sha256sum; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "Required tool is unavailable: ${tool}" >&2
    exit 69
  }
done

openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:3072 -out "$private_key" >/dev/null 2>&1
chmod 600 "$private_key"
openssl pkey -in "$private_key" -pubout -outform DER -out "$public_der"
nonce="$(openssl rand -hex 24)"
fingerprint="$(sha256sum "$public_der" | awk '{print $1}')"
public_key_b64="$(base64 --wrap=0 "$public_der")"

jq -nc \
  --arg run_id "$GITHUB_RUN_ID" \
  --arg nonce "$nonce" \
  --arg fingerprint "$fingerprint" \
  --arg public_key "$public_key_b64" \
  '{body:(
    "Encrypted GraphQL-only credential handoff requested for workflow run `" + $run_id + "`.\n\n" +
    "The user REST quota is not used. Only RSA-OAEP/SHA-256 ciphertext may be posted.\n\n" +
    "`graphql-public-key-v1:" + $nonce + ":" + $fingerprint + ":" + $public_key + "`"
  )}' > "$work/request-comment.json"
request_comment_id="$(issue_api POST "/issues/${TRACKING_ISSUE}/comments" "$work/request-comment.json" | jq -er '.id')"
echo "Published GraphQL handoff request as comment ${request_comment_id}."
unset public_key_b64

prefix="graphql-envelope-v1:${nonce}:"
deadline=$((SECONDS + HANDOFF_TIMEOUT_SECONDS))
ciphertext_b64=''
while (( SECONDS < deadline )); do
  issue_api GET "/issues/${TRACKING_ISSUE}/comments?per_page=100" > "$work/comments.json"
  record="$(
    jq -r --arg prefix "$prefix" '
      [.[] | select(.user.login == "ORESoftware") | select(.body | startswith($prefix)) | {id,body}]
      | last // empty | @base64
    ' "$work/comments.json"
  )"
  if [[ -n "$record" ]]; then
    decoded="$(printf '%s' "$record" | base64 --decode)"
    envelope_comment_id="$(jq -er '.id' <<<"$decoded")"
    body="$(jq -er '.body' <<<"$decoded")"
    ciphertext_b64="$(printf '%s' "${body#${prefix}}" | head -n1 | tr -d '[:space:]')"
    [[ "$ciphertext_b64" =~ ^[A-Za-z0-9+/]+={0,2}$ ]] && break
    envelope_comment_id=''
    ciphertext_b64=''
  fi
  sleep 3
done
[[ -n "$ciphertext_b64" ]] || { echo 'Encrypted GraphQL handoff expired.' >&2; exit 70; }

printf '%s' "$ciphertext_b64" | base64 --decode > "$work/token.enc"
openssl pkeyutl \
  -decrypt \
  -inkey "$private_key" \
  -pkeyopt rsa_padding_mode:oaep \
  -pkeyopt rsa_oaep_md:sha256 \
  -in "$work/token.enc" \
  -out "$work/token.txt"
github_token="$(cat "$work/token.txt")"
: > "$work/token.txt"
unset ciphertext_b64 body decoded record
[[ ${#github_token} -ge 20 && "$github_token" != *[[:space:]]* ]] || {
  echo 'Decrypted credential failed structural validation.' >&2
  exit 71
}
echo "::add-mask::$github_token"

delete_comment "$envelope_comment_id"
envelope_comment_id=''
delete_comment "$request_comment_id"
request_comment_id=''

graphql() {
  local query="$1" variables_json="$2" output="$3"
  jq -nc --arg query "$query" --argjson variables "$variables_json" \
    '{query:$query,variables:$variables}' > "$work/graphql-request.json"
  curl --silent --show-error --fail-with-body \
    --request POST \
    --header 'Accept: application/vnd.github+json' \
    --header "Authorization: Bearer ${github_token}" \
    --header "X-GitHub-Api-Version: ${API_VERSION}" \
    --header 'Content-Type: application/json' \
    --data-binary "@$work/graphql-request.json" \
    https://api.github.com/graphql > "$output"
  if jq -e '.errors and (.errors | length > 0)' "$output" >/dev/null; then
    jq -r '.errors[] | .message' "$output" >&2
    return 1
  fi
}

identity_query='query($login:String!){viewer{login} organization(login:$login){id}}'
graphql "$identity_query" "$(jq -nc --arg login "$TARGET_ORG" '{login:$login}')" "$work/identity.json"
actor="$(jq -er '.data.viewer.login' "$work/identity.json")"
org_id="$(jq -er '.data.organization.id' "$work/identity.json")"
[[ "$actor" == "$EXPECTED_LOGIN" && -n "$org_id" ]] || {
  echo 'Unexpected GraphQL identity or inaccessible organization.' >&2
  exit 72
}

ensure_repository() {
  local name="$1" description="$2"
  local lookup_query create_mutation variables
  lookup_query='query($owner:String!,$name:String!){repository(owner:$owner,name:$name){id nameWithOwner url}}'
  variables="$(jq -nc --arg owner "$TARGET_ORG" --arg name "$name" '{owner:$owner,name:$name}')"
  graphql "$lookup_query" "$variables" "$work/repository.json"
  if [[ "$(jq -r '.data.repository.id // empty' "$work/repository.json")" == '' ]]; then
    create_mutation='mutation($ownerId:ID!,$name:String!,$description:String!){createRepository(input:{ownerId:$ownerId,name:$name,description:$description,visibility:PUBLIC,hasIssuesEnabled:true,hasWikiEnabled:false}){repository{id nameWithOwner url}}}'
    variables="$(jq -nc --arg ownerId "$org_id" --arg name "$name" --arg description "$description" '{ownerId:$ownerId,name:$name,description:$description}')"
    graphql "$create_mutation" "$variables" "$work/repository.json"
  fi
  jq -er '.data.repository.nameWithOwner // .data.createRepository.repository.nameWithOwner' "$work/repository.json" >/dev/null
  printf 'REPOSITORY_READY %s/%s\n' "$TARGET_ORG" "$name"
}

ensure_repository des-web-playwright-e2e \
  'Playwright browser contracts for des-web across GitHub Actions and gha-indie-worker.'
ensure_repository des-web-puppeteer-e2e \
  'Puppeteer browser contracts for des-web across GitHub Actions and gha-indie-worker.'
ensure_repository .github \
  'Organization profile and DES browser-test fleet documentation.'

project_result='created-or-existing'
ensure_project() {
  local owner="$1" variables owner_id existing project_query create_mutation
  project_query='query($login:String!){organization(login:$login){id projectsV2(first:100){nodes{id number title}}}}'
  variables="$(jq -nc --arg login "$owner" '{login:$login}')"
  if ! graphql "$project_query" "$variables" "$work/project-${owner}.json"; then
    printf 'PROJECT_PENDING %s insufficient GraphQL project access\n' "$owner"
    project_result='pending'
    return 0
  fi
  owner_id="$(jq -er '.data.organization.id' "$work/project-${owner}.json")"
  existing="$(jq -r --arg title "$PROJECT_TITLE" '.data.organization.projectsV2.nodes[] | select(.title==$title) | .number' "$work/project-${owner}.json" | head -1)"
  if [[ -z "$existing" ]]; then
    create_mutation='mutation($ownerId:ID!,$title:String!){createProjectV2(input:{ownerId:$ownerId,title:$title}){projectV2{id number title}}}'
    variables="$(jq -nc --arg ownerId "$owner_id" --arg title "$PROJECT_TITLE" '{ownerId:$ownerId,title:$title}')"
    if ! graphql "$create_mutation" "$variables" "$work/project-${owner}.json"; then
      printf 'PROJECT_PENDING %s project creation failed\n' "$owner"
      project_result='pending'
      return 0
    fi
    existing="$(jq -er '.data.createProjectV2.projectV2.number' "$work/project-${owner}.json")"
  fi
  printf 'PROJECT_READY %s https://github.com/orgs/%s/projects/%s\n' "$owner" "$owner" "$existing"
}

ensure_project discrete-event-systems
ensure_project discrete-event-systems-test

jq -nc --arg actor "$actor" --arg project_result "$project_result" '{body:(
  "GraphQL-only repository publication completed.\n\n" +
  "- Authenticated actor: `" + $actor + "`\n" +
  "- Repositories created or verified: **3**\n" +
  "- Project result: `" + $project_result + "`\n" +
  "- User REST quota consumed: **0**\n\n" +
  "The ephemeral private key, plaintext credential, and handoff comments were removed."
)}' > "$work/success-comment.json"
issue_api POST "/issues/${TRACKING_ISSUE}/comments" "$work/success-comment.json" >/dev/null
printf 'DES_BROWSER_GRAPHQL_PUBLICATION_COMPLETE actor=%s project_result=%s\n' "$actor" "$project_result"

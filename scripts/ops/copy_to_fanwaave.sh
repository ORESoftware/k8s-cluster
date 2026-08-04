#!/usr/bin/env bash
set -Eeuo pipefail

umask 077

required_environment=(
  COMMENT_TOKEN
  DESTINATION_NAME
  DESTINATION_ORGANIZATION
  DESTINATION_REPOSITORY
  EXPECTED_LOGIN
  GITHUB_EVENT_PATH
  GITHUB_REPOSITORY
  GITHUB_RUN_ID
  GH_CONFIG_DIR
  SOURCE_REPOSITORY
  SOURCE_REPOSITORY_ID
  TRACKING_ISSUE
)
for variable in "${required_environment[@]}"; do
  if [[ -z "${!variable:-}" ]]; then
    printf 'required environment variable is missing: %s\n' "$variable" >&2
    exit 1
  fi
done

if [[ "$GITHUB_REPOSITORY" != "$SOURCE_REPOSITORY" ]]; then
  printf 'unexpected workflow repository: %s\n' "$GITHUB_REPOSITORY" >&2
  exit 1
fi

jq -e \
  --arg login "$EXPECTED_LOGIN" \
  --arg command "/copy-to-fanwaave" \
  --argjson issue "$TRACKING_ISSUE" \
  '.issue.number == $issue
   and .comment.user.login == $login
   and .comment.author_association == "OWNER"
   and .comment.body == $command' \
  "$GITHUB_EVENT_PATH" >/dev/null

source_json="$(GH_TOKEN="$COMMENT_TOKEN" gh api "repos/${SOURCE_REPOSITORY}")"
test "$(jq -er '.id | tostring' <<<"$source_json")" = "$SOURCE_REPOSITORY_ID"
test "$(jq -er .visibility <<<"$source_json")" = public
test "$(jq -er .default_branch <<<"$source_json")" = main
source_main="$(
  GH_TOKEN="$COMMENT_TOKEN" gh api \
    "repos/${SOURCE_REPOSITORY}/git/ref/heads/main" \
    --jq .object.sha
)"
[[ "$source_main" =~ ^[0-9a-f]{40}$ ]]
test "$(git rev-parse HEAD)" = "$source_main"

auth_log="$RUNNER_TEMP/fanwaave-copy-owner-auth.log"
auth_pid=''
auth_comment_id=''
askpass=''

cleanup() {
  if [[ -n "${auth_pid:-}" ]] && kill -0 "$auth_pid" 2>/dev/null; then
    kill "$auth_pid" 2>/dev/null || true
    wait "$auth_pid" 2>/dev/null || true
  fi
  if [[ -n "${auth_comment_id:-}" ]]; then
    GH_TOKEN="$COMMENT_TOKEN" gh api --method DELETE \
      "repos/${SOURCE_REPOSITORY}/issues/comments/${auth_comment_id}" \
      >/dev/null 2>&1 || true
  fi
  unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN token
  rm -f "$auth_log" "${askpass:-}"
  rm -rf "$GH_CONFIG_DIR"
}
trap cleanup EXIT

rm -rf "$GH_CONFIG_DIR"
mkdir -p "$GH_CONFIG_DIR"
: > "$auth_log"

(
  env -u GH_TOKEN -u GITHUB_TOKEN \
    GH_PROMPT_DISABLED=1 \
    NO_COLOR=1 \
    BROWSER=/bin/false \
    gh auth login \
      --hostname github.com \
      --git-protocol https \
      --web \
      --scopes repo,read:org \
      --insecure-storage
) > "$auth_log" 2>&1 &
auth_pid=$!

device_code=''
for _ in $(seq 1 45); do
  device_code="$(
    tr -d '\r' < "$auth_log" |
      grep -Eo '[A-Z0-9]{4}-[A-Z0-9]{4}' |
      head -n1 || true
  )"
  [[ -z "$device_code" ]] || break
  kill -0 "$auth_pid" 2>/dev/null || break
  sleep 1
done

if [[ -z "$device_code" ]]; then
  set +e
  wait "$auth_pid"
  auth_status=$?
  set -e
  auth_pid=''
  sed -E 's/[A-Z0-9]{4}-[A-Z0-9]{4}/[REDACTED-CODE]/g' "$auth_log" >&2 || true
  exit "${auth_status:-1}"
fi

authorization_body="**Fanwaave repository-copy authorization:** open https://github.com/login/device and enter **\`${device_code}\`**. This bounded run creates only \`${DESTINATION_REPOSITORY}\`, copies source branches and tags without deleting source refs, creates a migration commit, and opens a destination pull request. Run: https://github.com/${SOURCE_REPOSITORY}/actions/runs/${GITHUB_RUN_ID}"
auth_comment_id="$(
  GH_TOKEN="$COMMENT_TOKEN" gh api --method POST \
    "repos/${SOURCE_REPOSITORY}/issues/${TRACKING_ISSUE}/comments" \
    -f body="$authorization_body" \
    --jq .id
)"
[[ "$auth_comment_id" =~ ^[0-9]+$ ]]
unset authorization_body
echo "::notice title=GitHub device authorization::Open https://github.com/login/device and enter $device_code"

set +e
wait "$auth_pid"
auth_status=$?
set -e
auth_pid=''
if [[ "$auth_status" -ne 0 ]]; then
  sed -E 's/[A-Z0-9]{4}-[A-Z0-9]{4}/[REDACTED-CODE]/g' "$auth_log" >&2 || true
  exit "$auth_status"
fi
rm -f "$auth_log"

if GH_TOKEN="$COMMENT_TOKEN" gh api --method DELETE \
  "repos/${SOURCE_REPOSITORY}/issues/comments/${auth_comment_id}" >/dev/null; then
  auth_comment_id=''
fi

test "$(env -u GH_TOKEN -u GITHUB_TOKEN gh api user --jq .login)" = "$EXPECTED_LOGIN"
membership="$(
  env -u GH_TOKEN -u GITHUB_TOKEN gh api \
    "user/memberships/orgs/${DESTINATION_ORGANIZATION}"
)"
test "$(jq -er .state <<<"$membership")" = active
test "$(jq -er .role <<<"$membership")" = admin

token="$(env -u GH_TOKEN -u GITHUB_TOKEN gh auth token)"
test -n "$token"
[[ "$token" != *$'\n'* ]]
[[ "$token" != *$'\r'* ]]
[[ "$token" != *$'\t'* ]]
[[ "$token" != *' '* ]]
echo "::add-mask::$token"
export GH_TOKEN="$token"
export GITHUB_TOKEN="$token"

current_source_main="$(
  gh api "repos/${SOURCE_REPOSITORY}/git/ref/heads/main" --jq .object.sha
)"
test "$current_source_main" = "$source_main"

destination_json="$RUNNER_TEMP/fanwaave-destination.json"
destination_error="$RUNNER_TEMP/fanwaave-destination.error"
if gh api "repos/${DESTINATION_REPOSITORY}" \
  > "$destination_json" 2> "$destination_error"; then
  test "$(jq -er .full_name < "$destination_json")" = "$DESTINATION_REPOSITORY"
  test "$(jq -er .visibility < "$destination_json")" = public
  test "$(jq -er .owner.login < "$destination_json")" = "$DESTINATION_ORGANIZATION"
else
  if ! grep -q 'HTTP 404' "$destination_error"; then
    cat "$destination_error" >&2
    exit 1
  fi
  gh api --method POST "orgs/${DESTINATION_ORGANIZATION}/repos" \
    -f name="$DESTINATION_NAME" \
    -f description='Provider-neutral Rust push notification delivery service for FCM, APNs, Expo, and Web Push' \
    -f visibility=public \
    -F private=false \
    -F has_issues=true \
    -F has_projects=false \
    -F has_wiki=false \
    -F auto_init=false \
    -F allow_squash_merge=true \
    -F allow_merge_commit=true \
    -F allow_rebase_merge=false \
    -F delete_branch_on_merge=true \
    > "$destination_json"
  test "$(jq -er .full_name < "$destination_json")" = "$DESTINATION_REPOSITORY"
fi
rm -f "$destination_error"

destination_id="$(jq -er '.id | tostring' < "$destination_json")"
[[ "$destination_id" =~ ^[0-9]+$ ]]

existing_main=''
if existing_main="$(
  gh api "repos/${DESTINATION_REPOSITORY}/git/ref/heads/main" \
    --jq .object.sha 2>/dev/null
)"; then
  test "$existing_main" = "$source_main"
fi

askpass="$RUNNER_TEMP/fanwaave-copy-askpass.sh"
cat > "$askpass" <<'ASKPASS'
#!/usr/bin/env sh
case "${1:-}" in
  *Username*) printf '%s\n' 'x-access-token' ;;
  *Password*) printf '%s\n' "${GITHUB_TOKEN:?token required}" ;;
  *) exit 1 ;;
esac
ASKPASS
chmod 700 "$askpass"
export GIT_ASKPASS="$askpass"
export GIT_ASKPASS_REQUIRE=force
export GIT_TERMINAL_PROMPT=0
export GIT_CONFIG_COUNT=1
export GIT_CONFIG_KEY_0=credential.helper
export GIT_CONFIG_VALUE_0=''

mirror="$RUNNER_TEMP/push-notification-server.git"
rm -rf "$mirror"
git clone --mirror "https://github.com/${SOURCE_REPOSITORY}.git" "$mirror"
test "$(git -C "$mirror" rev-parse refs/heads/main)" = "$source_main"
git -C "$mirror" remote add fanwaave \
  "https://github.com/${DESTINATION_REPOSITORY}.git"
git -C "$mirror" push --porcelain fanwaave \
  'refs/heads/*:refs/heads/*'
git -C "$mirror" push --porcelain fanwaave \
  'refs/tags/*:refs/tags/*'

gh api --method PATCH "repos/${DESTINATION_REPOSITORY}" \
  -f default_branch=main \
  -F allow_squash_merge=true \
  -F allow_merge_commit=true \
  -F allow_rebase_merge=false \
  -F delete_branch_on_merge=true \
  >/dev/null

destination_main="$(
  gh api "repos/${DESTINATION_REPOSITORY}/git/ref/heads/main" \
    --jq .object.sha
)"
test "$destination_main" = "$source_main"

migration_branch='agent/establish-fanwaave-home'
migration_branch_sha=''
if migration_branch_sha="$(
  gh api "repos/${DESTINATION_REPOSITORY}/git/ref/heads/${migration_branch}" \
    --jq .object.sha 2>/dev/null
)"; then
  test "$migration_branch_sha" = "$source_main"
else
  gh api --method POST "repos/${DESTINATION_REPOSITORY}/git/refs" \
    -f ref="refs/heads/${migration_branch}" \
    -f sha="$source_main" \
    >/dev/null
fi

migration_file="$RUNNER_TEMP/fanwaave-repository-copy.md"
cat > "$migration_file" <<EOF
# Fanwaave repository home

This repository was copied from \`${SOURCE_REPOSITORY}\` after Fanwaave was selected as the canonical product organization.

- Source repository ID: \`${SOURCE_REPOSITORY_ID}\`
- Source \`main\` at copy: \`${source_main}\`
- Destination repository ID: \`${destination_id}\`
- Destination: \`${DESTINATION_REPOSITORY}\`

Source branches and tags were copied without deleting or rewriting the source repository. Future product work should target the Fanwaave repository.
EOF

encoded_content="$(base64 -w0 < "$migration_file")"
existing_content_json="$RUNNER_TEMP/fanwaave-copy-existing-content.json"
if gh api --method GET \
  "repos/${DESTINATION_REPOSITORY}/contents/docs/fanwaave-repository-copy.md" \
  -f ref="$migration_branch" \
  > "$existing_content_json" 2>/dev/null; then
  existing_file="$RUNNER_TEMP/fanwaave-copy-existing.md"
  jq -r .content < "$existing_content_json" |
    tr -d '\n' |
    base64 -d > "$existing_file"
  if ! cmp -s "$migration_file" "$existing_file"; then
    content_sha="$(jq -er .sha < "$existing_content_json")"
    gh api --method PUT \
      "repos/${DESTINATION_REPOSITORY}/contents/docs/fanwaave-repository-copy.md" \
      -f message='Establish the Fanwaave repository home' \
      -f content="$encoded_content" \
      -f branch="$migration_branch" \
      -f sha="$content_sha" \
      >/dev/null
  fi
else
  gh api --method PUT \
    "repos/${DESTINATION_REPOSITORY}/contents/docs/fanwaave-repository-copy.md" \
    -f message='Establish the Fanwaave repository home' \
    -f content="$encoded_content" \
    -f branch="$migration_branch" \
    >/dev/null
fi

pr_url="$(
  gh api --method GET "repos/${DESTINATION_REPOSITORY}/pulls" \
    -f state=open \
    -f head="${DESTINATION_ORGANIZATION}:${migration_branch}" \
    --jq '.[0].html_url // empty'
)"
if [[ -z "$pr_url" ]]; then
  pr_body="$RUNNER_TEMP/fanwaave-pr-body.md"
  cat > "$pr_body" <<EOF
## What changed

- copied the complete source branch and tag history from \`${SOURCE_REPOSITORY}\`
- retained \`main\` at source commit \`${source_main}\`
- added migration provenance for the Fanwaave repository

## Why

Fanwaave is the canonical product organization for the Rust push notification server. The source repository remains intact while this pull request establishes the destination as an independently reviewable GitHub repository.

## Validation

- destination repository ID: \`${destination_id}\`
- destination \`main\` equals the bound source \`main\`
- repository creation and pushes used an ephemeral owner-authorized OAuth token; no PAT was stored
EOF
  pr_url="$(
    gh api --method POST "repos/${DESTINATION_REPOSITORY}/pulls" \
      -f title='Establish the Fanwaave repository home' \
      -f head="$migration_branch" \
      -f base=main \
      -F maintainer_can_modify=true \
      -f body="$(cat "$pr_body")" \
      --jq .html_url
  )"
fi

completion_body="Copied \`${SOURCE_REPOSITORY}\` to [\`${DESTINATION_REPOSITORY}\`](https://github.com/${DESTINATION_REPOSITORY}) without deleting the source. Destination repository ID: \`${destination_id}\`. Migration PR: ${pr_url}. Source main copied: \`${source_main}\`."
GH_TOKEN="$COMMENT_TOKEN" gh api --method POST \
  "repos/${SOURCE_REPOSITORY}/issues/${TRACKING_ISSUE}/comments" \
  -f body="$completion_body" \
  >/dev/null

printf 'destination_repository=%s\n' "$DESTINATION_REPOSITORY"
printf 'destination_repository_id=%s\n' "$destination_id"
printf 'source_main=%s\n' "$source_main"
printf 'migration_pr=%s\n' "$pr_url"

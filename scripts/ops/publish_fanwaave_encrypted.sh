#!/usr/bin/env bash
set -Eeuo pipefail

umask 077

required_environment=(
  COMMENT_TOKEN
  DESTINATION_ORGANIZATION
  EXPECTED_LOGIN
  GITHUB_EVENT_PATH
  GITHUB_REPOSITORY
  GITHUB_RUN_ID
  PROJECT_TITLE
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

trigger_command='/publish-fanwaave'
token_command='/fanwaave-token'
jq -e \
  --arg login "$EXPECTED_LOGIN" \
  --arg command "$trigger_command" \
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

work_root="$RUNNER_TEMP/fanwaave-encrypted-publication"
private_key="$work_root/private.pem"
public_der="$work_root/public.der"
ciphertext_file="$work_root/token.bin"
token_file="$work_root/token"
key_comment_id=''
token_comment_id=''
askpass=''

cleanup() {
  if [[ -n "${key_comment_id:-}" ]]; then
    GH_TOKEN="$COMMENT_TOKEN" gh api --method DELETE \
      "repos/${SOURCE_REPOSITORY}/issues/comments/${key_comment_id}" \
      >/dev/null 2>&1 || true
  fi
  if [[ -n "${token_comment_id:-}" ]]; then
    GH_TOKEN="$COMMENT_TOKEN" gh api --method DELETE \
      "repos/${SOURCE_REPOSITORY}/issues/comments/${token_comment_id}" \
      >/dev/null 2>&1 || true
  fi
  unset GH_TOKEN GITHUB_TOKEN token
  rm -rf "$work_root"
}
trap cleanup EXIT

rm -rf "$work_root"
mkdir -p "$work_root"

openssl genpkey \
  -algorithm RSA \
  -pkeyopt rsa_keygen_bits:3072 \
  -out "$private_key" \
  >/dev/null 2>&1
openssl pkey \
  -in "$private_key" \
  -pubout \
  -outform DER \
  -out "$public_der"
public_key_base64="$(base64 -w0 < "$public_der")"
handshake="$(openssl rand -hex 16)"
[[ "$handshake" =~ ^[0-9a-f]{32}$ ]]

handoff_body="**Encrypted Fanwaave publisher handoff:** handshake **\`${handshake}\`**. RSA-3072 public key (DER, base64): **\`${public_key_base64}\`**. Encrypt the already-authorized GitHub token with RSA-OAEP SHA-256 and post exactly: \`${token_command} ${handshake} <ciphertext-base64>\`. The private key exists only inside this Actions runner and is destroyed at exit. Run: https://github.com/${SOURCE_REPOSITORY}/actions/runs/${GITHUB_RUN_ID}"
key_comment_id="$(
  GH_TOKEN="$COMMENT_TOKEN" gh api --method POST \
    "repos/${SOURCE_REPOSITORY}/issues/${TRACKING_ISSUE}/comments" \
    -f body="$handoff_body" \
    --jq .id
)"
[[ "$key_comment_id" =~ ^[0-9]+$ ]]
unset handoff_body public_key_base64
echo "::notice title=Encrypted GitHub token handoff::Handshake $handshake is ready on source issue $TRACKING_ISSUE"

token_body=''
for _ in $(seq 1 300); do
  comments="$(
    GH_TOKEN="$COMMENT_TOKEN" gh api \
      "repos/${SOURCE_REPOSITORY}/issues/${TRACKING_ISSUE}/comments?per_page=100"
  )"
  token_record="$(
    jq -c \
      --arg login "$EXPECTED_LOGIN" \
      --arg prefix "${token_command} ${handshake} " \
      '[.[]
        | select(.user.login == $login)
        | select(.author_association == "OWNER")
        | select(.body | startswith($prefix))]
       | last // empty' \
      <<<"$comments"
  )"
  if [[ -n "$token_record" ]]; then
    token_comment_id="$(jq -er '.id | tostring' <<<"$token_record")"
    token_body="$(jq -er .body <<<"$token_record")"
    break
  fi
  sleep 3
done

if [[ -z "$token_body" ]]; then
  printf 'encrypted token handoff timed out\n' >&2
  exit 1
fi

token_prefix="${token_command} ${handshake} "
ciphertext_base64="${token_body#"$token_prefix"}"
test "$ciphertext_base64" != "$token_body"
[[ "$ciphertext_base64" =~ ^[A-Za-z0-9+/=]+$ ]]
printf '%s' "$ciphertext_base64" | base64 -d > "$ciphertext_file"
openssl pkeyutl \
  -decrypt \
  -inkey "$private_key" \
  -in "$ciphertext_file" \
  -pkeyopt rsa_padding_mode:oaep \
  -pkeyopt rsa_oaep_md:sha256 \
  -pkeyopt rsa_mgf1_md:sha256 \
  > "$token_file"
chmod 600 "$token_file"

token="$(cat "$token_file")"
test -n "$token"
[[ "$token" != *$'\n'* ]]
[[ "$token" != *$'\r'* ]]
[[ "$token" != *$'\t'* ]]
[[ "$token" != *' '* ]]
echo "::add-mask::$token"

if GH_TOKEN="$COMMENT_TOKEN" gh api --method DELETE \
  "repos/${SOURCE_REPOSITORY}/issues/comments/${token_comment_id}" >/dev/null; then
  token_comment_id=''
fi
if GH_TOKEN="$COMMENT_TOKEN" gh api --method DELETE \
  "repos/${SOURCE_REPOSITORY}/issues/comments/${key_comment_id}" >/dev/null; then
  key_comment_id=''
fi

export GH_TOKEN="$token"
export GITHUB_TOKEN="$token"

test "$(gh api user --jq .login)" = "$EXPECTED_LOGIN"
membership="$(gh api "user/memberships/orgs/${DESTINATION_ORGANIZATION}")"
test "$(jq -er .state <<<"$membership")" = active
test "$(jq -er .role <<<"$membership")" = admin

current_source_main="$(
  gh api "repos/${SOURCE_REPOSITORY}/git/ref/heads/main" --jq .object.sha
)"
test "$current_source_main" = "$source_main"

askpass="$work_root/askpass.sh"
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

push_repository="${DESTINATION_ORGANIZATION}/push-notification-server.rs"
push_json_file="$work_root/push-repository.json"
push_error_file="$work_root/push-repository.error"
if gh api "repos/${push_repository}" > "$push_json_file" 2> "$push_error_file"; then
  test "$(jq -er .full_name < "$push_json_file")" = "$push_repository"
  test "$(jq -er .visibility < "$push_json_file")" = public
  test "$(jq -er .owner.login < "$push_json_file")" = "$DESTINATION_ORGANIZATION"
else
  if ! grep -q 'HTTP 404' "$push_error_file"; then
    cat "$push_error_file" >&2
    exit 1
  fi
  gh api --method POST "orgs/${DESTINATION_ORGANIZATION}/repos" \
    -f name='push-notification-server.rs' \
    -f description='Provider-neutral Rust push notification delivery service for FCM, APNs, Expo, Web Push, SendGrid, and Twilio' \
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
    > "$push_json_file"
  test "$(jq -er .full_name < "$push_json_file")" = "$push_repository"
fi
rm -f "$push_error_file"
push_repository_id="$(jq -er '.id | tostring' < "$push_json_file")"
[[ "$push_repository_id" =~ ^[0-9]+$ ]]

mirror="$work_root/push-notification-server.git"
git clone --mirror "https://github.com/${SOURCE_REPOSITORY}.git" "$mirror"
test "$(git -C "$mirror" rev-parse refs/heads/main)" = "$source_main"
git -C "$mirror" remote add fanwaave \
  "https://github.com/${push_repository}.git"

existing_push_main=''
if existing_push_main="$(
  gh api "repos/${push_repository}/git/ref/heads/main" \
    --jq .object.sha 2>/dev/null
)"; then
  git -C "$mirror" fetch fanwaave \
    "refs/heads/main:refs/remotes/fanwaave/main"
  git -C "$mirror" merge-base --is-ancestor \
    "$existing_push_main" "$source_main"
fi

git -C "$mirror" push --porcelain fanwaave \
  'refs/heads/*:refs/heads/*'
git -C "$mirror" push --porcelain fanwaave \
  'refs/tags/*:refs/tags/*'

gh api --method PATCH "repos/${push_repository}" \
  -f default_branch=main \
  -F allow_squash_merge=true \
  -F allow_merge_commit=true \
  -F allow_rebase_merge=false \
  -F delete_branch_on_merge=true \
  >/dev/null

destination_main="$(
  gh api "repos/${push_repository}/git/ref/heads/main" \
    --jq .object.sha
)"
test "$destination_main" = "$source_main"

migration_branch='agent/establish-fanwaave-home'
if ! gh api "repos/${push_repository}/git/ref/heads/${migration_branch}" \
  >/dev/null 2>&1; then
  gh api --method POST "repos/${push_repository}/git/refs" \
    -f ref="refs/heads/${migration_branch}" \
    -f sha="$source_main" \
    >/dev/null
fi

migration_file="$work_root/fanwaave-repository-copy.md"
cat > "$migration_file" <<EOF
# Fanwaave repository home

This repository was copied from \`${SOURCE_REPOSITORY}\` after Fanwaave was selected as the canonical product organization.

- Source repository ID: \`${SOURCE_REPOSITORY_ID}\`
- Source \`main\` at publication: \`${source_main}\`
- Destination repository ID: \`${push_repository_id}\`
- Destination: \`${push_repository}\`

Source branches and tags were copied without deleting, rewriting, archiving, or force-pushing the source repository. Future product work should target the Fanwaave repository. The publication used an ephemeral encrypted credential handoff whose private key was destroyed inside the bounded Actions runner.
EOF

encoded_content="$(base64 -w0 < "$migration_file")"
existing_content_json="$work_root/existing-copy-content.json"
if gh api --method GET \
  "repos/${push_repository}/contents/docs/fanwaave-repository-copy.md" \
  -f ref="$migration_branch" \
  > "$existing_content_json" 2>/dev/null; then
  existing_file="$work_root/existing-copy.md"
  jq -r .content < "$existing_content_json" |
    tr -d '\n' |
    base64 -d > "$existing_file"
  if ! cmp -s "$migration_file" "$existing_file"; then
    content_sha="$(jq -er .sha < "$existing_content_json")"
    gh api --method PUT \
      "repos/${push_repository}/contents/docs/fanwaave-repository-copy.md" \
      -f message='Establish the Fanwaave repository home' \
      -f content="$encoded_content" \
      -f branch="$migration_branch" \
      -f sha="$content_sha" \
      >/dev/null
  fi
else
  gh api --method PUT \
    "repos/${push_repository}/contents/docs/fanwaave-repository-copy.md" \
    -f message='Establish the Fanwaave repository home' \
    -f content="$encoded_content" \
    -f branch="$migration_branch" \
    >/dev/null
fi

push_pr_url="$(
  gh api --method GET "repos/${push_repository}/pulls" \
    -f state=open \
    -f head="${DESTINATION_ORGANIZATION}:${migration_branch}" \
    --jq '.[0].html_url // empty'
)"
if [[ -z "$push_pr_url" ]]; then
  push_pr_body="$work_root/push-pr-body.md"
  cat > "$push_pr_body" <<EOF
## What changed

- copied the complete source branch and tag history from \`${SOURCE_REPOSITORY}\`
- retained \`main\` at source commit \`${source_main}\`
- added migration provenance for the Fanwaave repository

## Why

Fanwaave is the canonical product organization for the Rust push notification server. The source repository remains intact while this pull request establishes the destination as an independently reviewable GitHub repository.

## Validation

- destination repository ID: \`${push_repository_id}\`
- destination \`main\` equals the bound source \`main\`
- publication used an ephemeral encrypted token handoff; no PAT was committed or stored
- Linear: https://linear.app/denman/issue/DEN-1874
EOF
  push_pr_url="$(
    gh api --method POST "repos/${push_repository}/pulls" \
      -f title='Establish the Fanwaave repository home' \
      -f head="$migration_branch" \
      -f base=main \
      -F maintainer_can_modify=true \
      -f body="$(cat "$push_pr_body")" \
      --jq .html_url
  )"
fi

governance_repository="${DESTINATION_ORGANIZATION}/.github"
governance_json_file="$work_root/governance-repository.json"
governance_error_file="$work_root/governance-repository.error"
if gh api "repos/${governance_repository}" \
  > "$governance_json_file" 2> "$governance_error_file"; then
  test "$(jq -er .full_name < "$governance_json_file")" = "$governance_repository"
  test "$(jq -er .visibility < "$governance_json_file")" = public
else
  if ! grep -q 'HTTP 404' "$governance_error_file"; then
    cat "$governance_error_file" >&2
    exit 1
  fi
  gh api --method POST "orgs/${DESTINATION_ORGANIZATION}/repos" \
    -f name='.github' \
    -f description='Public organization profile, governance, and GitHub/Linear operating context for Fanwaave' \
    -f visibility=public \
    -F private=false \
    -F has_issues=true \
    -F has_projects=false \
    -F has_wiki=false \
    -F auto_init=true \
    -F allow_squash_merge=true \
    -F allow_merge_commit=true \
    -F allow_rebase_merge=false \
    -F delete_branch_on_merge=true \
    > "$governance_json_file"
  test "$(jq -er .full_name < "$governance_json_file")" = "$governance_repository"
fi
rm -f "$governance_error_file"
governance_repository_id="$(
  jq -er '.id | tostring' < "$governance_json_file"
)"
[[ "$governance_repository_id" =~ ^[0-9]+$ ]]

if ! gh api "repos/${governance_repository}/git/ref/heads/main" \
  >/dev/null 2>&1; then
  initial_readme="$(printf '# Fanwaave organization governance\n' | base64 -w0)"
  gh api --method PUT \
    "repos/${governance_repository}/contents/README.md" \
    -f message='Initialize Fanwaave organization governance' \
    -f content="$initial_readme" \
    >/dev/null
fi

organization_projects="$(
  gh api graphql \
    -f query='query($login:String!){organization(login:$login){id projectsV2(first:100){nodes{id number title url}}}}' \
    -f login="$DESTINATION_ORGANIZATION"
)"
organization_id="$(jq -er .data.organization.id <<<"$organization_projects")"
project_record="$(
  jq -c --arg title "$PROJECT_TITLE" \
    '.data.organization.projectsV2.nodes[] | select(.title == $title)' \
    <<<"$organization_projects" | head -n1
)"
if [[ -z "$project_record" ]]; then
  project_record="$(
    gh api graphql \
      -f query='mutation($ownerId:ID!,$title:String!){createProjectV2(input:{ownerId:$ownerId,title:$title}){projectV2{id number title url}}}' \
      -F ownerId="$organization_id" \
      -f title="$PROJECT_TITLE" |
      jq -cer .data.createProjectV2.projectV2
  )"
fi
project_id="$(jq -er .id <<<"$project_record")"
project_url="$(jq -er .url <<<"$project_record")"
project_number="$(jq -er '.number | tostring' <<<"$project_record")"

project_readme="$work_root/project-readme.md"
cat > "$project_readme" <<EOF
# Fanwaave delivery platform

Canonical planning board for the Fanwaave GitHub organization.

- Linear project: https://linear.app/denman/project/githubcomfanwaave-6ba038b59dbc
- Migration issue: https://linear.app/denman/issue/DEN-1874
- Reliability issue: https://linear.app/denman/issue/DEN-1875
- Push server: https://github.com/${push_repository}
- Organization governance: https://github.com/${governance_repository}

Track repository migration, provider adapters and receipts, shared-auth/Supabase integration, the Postgres communications ledger, NATS and Kubernetes deployment, documentation, security, and release readiness here.
EOF

if ! gh api graphql \
  -f query='mutation($projectId:ID!,$shortDescription:String!,$readme:String!){updateProjectV2(input:{projectId:$projectId,shortDescription:$shortDescription,readme:$readme,public:true}){projectV2{id url}}}' \
  -F projectId="$project_id" \
  -f shortDescription='Fanwaave repository, provider, platform, security, and release roadmap' \
  -f readme="$(cat "$project_readme")" \
  >/dev/null; then
  gh api graphql \
    -f query='mutation($projectId:ID!,$shortDescription:String!,$readme:String!){updateProjectV2(input:{projectId:$projectId,shortDescription:$shortDescription,readme:$readme}){projectV2{id url}}}' \
    -F projectId="$project_id" \
    -f shortDescription='Fanwaave repository, provider, platform, security, and release roadmap' \
    -f readme="$(cat "$project_readme")" \
    >/dev/null
fi

governance_checkout="$work_root/governance"
git clone "https://github.com/${governance_repository}.git" "$governance_checkout"
git -C "$governance_checkout" config user.name 'github-actions[bot]'
git -C "$governance_checkout" config user.email \
  '41898282+github-actions[bot]@users.noreply.github.com'
git -C "$governance_checkout" fetch origin main

governance_branch='agent/establish-org-governance'
if git -C "$governance_checkout" ls-remote --exit-code --heads \
  origin "$governance_branch" >/dev/null 2>&1; then
  git -C "$governance_checkout" fetch origin "$governance_branch"
  git -C "$governance_checkout" checkout -B \
    "$governance_branch" "origin/${governance_branch}"
  git -C "$governance_checkout" merge --no-edit origin/main
else
  git -C "$governance_checkout" checkout -B \
    "$governance_branch" origin/main
fi

mkdir -p \
  "$governance_checkout/profile" \
  "$governance_checkout/docs" \
  "$governance_checkout/.github/ISSUE_TEMPLATE" \
  "$governance_checkout/.github/workflows"

cat > "$governance_checkout/README.md" <<EOF
# Fanwaave organization governance

This special public repository provides Fanwaave's organization profile, community-health defaults, repository relationships, security guidance, and the canonical GitHub/Linear operating map.

- Organization: https://github.com/${DESTINATION_ORGANIZATION}
- Push server: https://github.com/${push_repository}
- GitHub Project: ${project_url}
- Linear project: https://linear.app/denman/project/githubcomfanwaave-6ba038b59dbc

Implementation work belongs in product repositories. Organization policy and shared templates belong here and land through reviewed pull requests.
EOF

cat > "$governance_checkout/profile/README.md" <<EOF
# Fanwaave

Fanwaave builds reliable communication and fan-engagement infrastructure.

## Canonical repositories

- [push-notification-server.rs](https://github.com/${push_repository}) — Rust notification delivery for FCM, APNs, Expo, Web Push, SendGrid, and Twilio lanes.
- [.github](https://github.com/${governance_repository}) — organization profile, governance, security, and shared project context.

## Planning

- [GitHub Project: ${PROJECT_TITLE}](${project_url})
- [Linear project: github.com/fanwaave](https://linear.app/denman/project/githubcomfanwaave-6ba038b59dbc)

Changes are developed on feature branches, reviewed in pull requests, validated on the exact head, and merged without rewriting shared history.
EOF

cat > "$governance_checkout/SECURITY.md" <<'EOF'
# Security policy

Report suspected vulnerabilities privately through GitHub Security Advisories in the affected Fanwaave repository. Do not publish credentials, device tokens, push endpoints, private keys, API keys, phone numbers, email addresses, or exploit details in public issues.

Provider credentials are server-side configuration supplied through managed secret systems. Repository source, CI logs, project boards, Linear, and issue comments must not contain live credentials.

Security fixes use private coordination until a patch, verification evidence, and disclosure plan are ready.
EOF

cat > "$governance_checkout/CONTRIBUTING.md" <<'EOF'
# Contributing

1. Start from the repository's current default branch.
2. Create a focused feature branch.
3. Preserve repository-local `AGENTS.md` and nested instructions.
4. Add tests and documentation for behavior changes.
5. Run the repository's formatting, lint, test, security, and build commands.
6. Open a pull request describing scope, risk, validation, and rollout.
7. Resolve conflicts semantically after reviewing the merge base and relevant history; do not select one side wholesale.
8. Merge only the reviewed exact head after required checks pass.

Never commit secrets or production customer data.
EOF

cat > "$governance_checkout/CODE_OF_CONDUCT.md" <<'EOF'
# Community conduct

Fanwaave collaboration must be respectful, constructive, and safe. Harassment, threats, discrimination, doxxing, credential exposure, and deliberate disruption are not acceptable.

Discuss technical disagreements with evidence and focus criticism on designs and changes rather than people. Maintainers may edit, hide, lock, or remove content and restrict participation when needed to protect contributors and users.

Report conduct concerns privately to organization maintainers.
EOF

cat > "$governance_checkout/SUPPORT.md" <<'EOF'
# Support

Use the affected repository's issue tracker for reproducible defects, documentation problems, and feature proposals. Use GitHub Security Advisories for vulnerabilities. Do not place secrets, private customer data, device tokens, push endpoints, phone numbers, or email addresses in public support requests.
EOF

cat > "$governance_checkout/PULL_REQUEST_TEMPLATE.md" <<'EOF'
## What changed

## Why

## Validation

## Risk and rollback

## Tracking

- Linear:
- GitHub Project:
EOF

cat > "$governance_checkout/.github/ISSUE_TEMPLATE/config.yml" <<'EOF'
blank_issues_enabled: true
contact_links:
  - name: Security vulnerability
    url: https://github.com/fanwaave/.github/security/advisories/new
    about: Report vulnerabilities privately rather than opening a public issue.
EOF

cat > "$governance_checkout/AGENTS.md" <<'EOF'
# Fanwaave organization instructions

- Work on feature branches and use pull requests.
- Do not rebase shared branches; merge semantically with full context.
- Inspect the merge base, both sides, path history, and 3–10 relevant commits when resolving conflicts.
- Never commit secrets, provider credentials, production device tokens, personal contact data, or capability-bearing push endpoints.
- Repository-local and path-specific instructions take precedence over this organization baseline.
EOF

cat > "$governance_checkout/docs/OPERATING_MODEL.md" <<EOF
# Fanwaave GitHub and Linear operating model

## Canonical systems

- GitHub organization: https://github.com/${DESTINATION_ORGANIZATION}
- GitHub Project: ${project_url}
- Linear project: https://linear.app/denman/project/githubcomfanwaave-6ba038b59dbc
- Migration issue: https://linear.app/denman/issue/DEN-1874
- Reliability issue: https://linear.app/denman/issue/DEN-1875

## Repository model

\`${push_repository}\` owns push and contact delivery implementation, provider adapters, contracts, CI, security, and release artifacts. \`${governance_repository}\` owns public organization context and shared community-health files.

## Delivery workflow

Changes move from Linear issue to feature branch to reviewed GitHub pull request to exact-head CI to merge. GitHub Project items provide the organization execution view; Linear remains the planning and cross-project source of record.
EOF

cat > "$governance_checkout/repository-relationships.json" <<EOF
{
  "schema_version": 1,
  "github": {
    "organization": "${DESTINATION_ORGANIZATION}",
    "organization_id": 311152026,
    "governance_repository": "${governance_repository}",
    "push_repository": "${push_repository}"
  },
  "linear": {
    "project_id": "d765e227-5726-42c8-8643-a8bd9e5a9a8c",
    "project_name": "github.com/fanwaave",
    "project_url": "https://linear.app/denman/project/githubcomfanwaave-6ba038b59dbc"
  },
  "github_project": {
    "id": "${project_id}",
    "number": ${project_number},
    "title": "${PROJECT_TITLE}",
    "url": "${project_url}"
  },
  "automatic_agent_instruction_inheritance": false,
  "unregistered_dependencies": "unknown_not_assumed"
}
EOF

cat > "$governance_checkout/.github/workflows/validate.yml" <<'EOF'
name: Validate organization governance

on:
  pull_request:
  push:
    branches: [main]

permissions:
  contents: read

jobs:
  validate:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
      - name: Validate JSON and required files
        shell: bash
        run: |
          set -euo pipefail
          python3 -m json.tool repository-relationships.json >/dev/null
          test -s README.md
          test -s profile/README.md
          test -s SECURITY.md
          test -s CONTRIBUTING.md
          test -s CODE_OF_CONDUCT.md
          test -s AGENTS.md
          test -s docs/OPERATING_MODEL.md
          grep -Fq 'github.com/fanwaave' profile/README.md
          grep -Fq 'linear.app/denman/project/githubcomfanwaave' docs/OPERATING_MODEL.md
EOF

git -C "$governance_checkout" add --all
if ! git -C "$governance_checkout" diff --cached --quiet; then
  git -C "$governance_checkout" diff --cached --check
  git -C "$governance_checkout" commit \
    -m 'Establish Fanwaave organization governance'
  git -C "$governance_checkout" push origin "$governance_branch"
fi

governance_pr_url="$(
  gh api --method GET "repos/${governance_repository}/pulls" \
    -f state=open \
    -f head="${DESTINATION_ORGANIZATION}:${governance_branch}" \
    --jq '.[0].html_url // empty'
)"
if [[ -z "$governance_pr_url" ]]; then
  governance_pr_body="$work_root/governance-pr-body.md"
  cat > "$governance_pr_body" <<EOF
## What changed

- publish the Fanwaave organization profile and governance repository
- add shared security, contributing, conduct, support, pull-request, and issue guidance
- record GitHub, Linear, and Projects v2 relationships
- add exact-head validation for the governance bundle

## Why

Fanwaave needs a public organization home and a durable operating map alongside the canonical Rust push-notification service.

## Validation

- organization repository ID: \`${governance_repository_id}\`
- GitHub Project: ${project_url}
- source tracking: https://github.com/${SOURCE_REPOSITORY}/issues/${TRACKING_ISSUE}
- publication used an ephemeral encrypted token handoff; no PAT was committed or stored
EOF
  governance_pr_url="$(
    gh api --method POST "repos/${governance_repository}/pulls" \
      -f title='Establish Fanwaave organization governance' \
      -f head="$governance_branch" \
      -f base=main \
      -F maintainer_can_modify=true \
      -f body="$(cat "$governance_pr_body")" \
      --jq .html_url
  )"
fi

ensure_issue() {
  local repository="$1"
  local title="$2"
  local body="$3"
  local existing
  existing="$(
    gh api "repos/${repository}/issues?state=all&per_page=100" |
      jq -c --arg title "$title" \
        '[.[] | select(.title == $title and (has("pull_request") | not))]
         | .[0] // empty'
  )"
  if [[ -n "$existing" ]]; then
    printf '%s\n' "$existing"
  else
    gh api --method POST "repos/${repository}/issues" \
      -f title="$title" \
      -f body="$body"
  fi
}

cutover_issue="$(
  ensure_issue "$push_repository" \
    'Complete the Fanwaave repository cutover' \
    'Track canonical migration, destination CI, app access, deployment references, source relocation notice, routing-registry merges, and release ownership. Linear: https://linear.app/denman/issue/DEN-1874'
)"
reliability_issue="$(
  ensure_issue "$push_repository" \
    'Harden provider delivery, retries, receipts, and recovery' \
    'Track FCM, APNs, Expo, Web Push, SendGrid, and Twilio isolation; retry classification; receipts; dead letters; observability; credential boundaries; and E2E recovery. Linear: https://linear.app/denman/issue/DEN-1875'
)"
deployment_issue="$(
  ensure_issue "$push_repository" \
    'Deploy the push service from the Fanwaave namespace' \
    'Update Kubernetes, images, secrets, webhooks, NATS subjects, service catalogs, submodules, and runbooks to use the canonical Fanwaave repository and release artifacts.'
)"
governance_issue="$(
  ensure_issue "$governance_repository" \
    'Maintain Fanwaave organization governance and project routing' \
    "Keep the organization profile, community-health defaults, repository relationship manifest, GitHub Project ${project_url}, and Linear project https://linear.app/denman/project/githubcomfanwaave-6ba038b59dbc synchronized."
)"

project_items="$(
  gh api graphql \
    -f query='query($projectId:ID!){node(id:$projectId){... on ProjectV2{items(first:100){nodes{content{... on Issue{id}}}}}}}' \
    -F projectId="$project_id"
)"

for issue_json in \
  "$cutover_issue" \
  "$reliability_issue" \
  "$deployment_issue" \
  "$governance_issue"; do
  issue_node_id="$(jq -er .node_id <<<"$issue_json")"
  if ! jq -e --arg id "$issue_node_id" \
    'any(.data.node.items.nodes[]; .content.id? == $id)' \
    <<<"$project_items" >/dev/null; then
    gh api graphql \
      -f query='mutation($projectId:ID!,$contentId:ID!){addProjectV2ItemById(input:{projectId:$projectId,contentId:$contentId}){item{id}}}' \
      -F projectId="$project_id" \
      -F contentId="$issue_node_id" \
      >/dev/null
  fi
done

completion_body="Published [\`${push_repository}\`](https://github.com/${push_repository}) and [\`${governance_repository}\`](https://github.com/${governance_repository}); created or reused GitHub Project [\`${PROJECT_TITLE}\`](${project_url}). Push migration PR: ${push_pr_url}. Governance PR: ${governance_pr_url}. Source \`main\` copied: \`${source_main}\`. No PAT was committed or stored."
GH_TOKEN="$COMMENT_TOKEN" gh api --method POST \
  "repos/${SOURCE_REPOSITORY}/issues/${TRACKING_ISSUE}/comments" \
  -f body="$completion_body" \
  >/dev/null

printf 'push_repository=%s\n' "$push_repository"
printf 'push_repository_id=%s\n' "$push_repository_id"
printf 'push_pull_request=%s\n' "$push_pr_url"
printf 'governance_repository=%s\n' "$governance_repository"
printf 'governance_repository_id=%s\n' "$governance_repository_id"
printf 'governance_pull_request=%s\n' "$governance_pr_url"
printf 'project_id=%s\n' "$project_id"
printf 'project_url=%s\n' "$project_url"
printf 'source_main=%s\n' "$source_main"

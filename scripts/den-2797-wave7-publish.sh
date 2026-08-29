#!/usr/bin/env bash
set -Eeuo pipefail
work=/tmp/den-2797-wave7
umask 077
askpass=/tmp/den-2797-wave7-askpass
cat > "$askpass" <<'EOF'
#!/bin/sh
case "$1" in
  *Username*) printf '%s\n' x-access-token ;;
  *Password*) printf '%s\n' "$GH_TOKEN" ;;
  *) printf '\n' ;;
esac
EOF
sed -i 's/^          //' "$askpass"
chmod 700 "$askpass"

graphql_retry() {
  local response error_file=/tmp/den-2797-wave7-graphql-error.txt
  for attempt in $(seq 1 40); do
    : > "$error_file"
    if response="$(gh api graphql "$@" 2>"$error_file")"; then
      if jq -e '.errors and (.errors | length > 0)' >/dev/null 2>&1 <<<"$response"; then
        jq -r '.errors[].message' <<<"$response" > "$error_file"
      else
        printf '%s' "$response"
        return 0
      fi
    fi
    error_text="$(cat "$error_file")"
    if grep -Eqi 'rate limit|secondary rate|abuse|temporarily unavailable|timeout' <<<"$error_text"; then
      sleep_for=$(( attempt * 15 ))
      (( sleep_for <= 120 )) || sleep_for=120
      echo "GraphQL temporarily unavailable; retry ${attempt}/40 in ${sleep_for}s" >&2
      sleep "$sleep_for"
      continue
    fi
    cat "$error_file" >&2
    return 1
  done
  echo 'GraphQL did not recover within the bounded retry window' >&2
  return 1
}

organization_id() {
  local organization="$1" response id login viewer
  response="$(graphql_retry \
    -f query='query($login:String!){viewer{login} organization(login:$login){id login}}' \
    -f login="$organization")"
  viewer="$(jq -r '.data.viewer.login // empty' <<<"$response")"
  id="$(jq -r '.data.organization.id // empty' <<<"$response")"
  login="$(jq -r '.data.organization.login // empty' <<<"$response")"
  test -n "$viewer"
  test -n "$id"
  test "$login" = "$organization"
  printf '%s' "$id"
}

query_repository() {
  local organization="$1" repository="$2"
  graphql_retry \
    -f query='query($login:String!,$name:String!){organization(login:$login){repository(name:$name){id nameWithOwner url visibility isArchived isDisabled defaultBranchRef{name target{oid}}}}}' \
    -f login="$organization" \
    -f name="$repository"
}

create_or_verify_empty() {
  local organization="$1" repository="$2" description="$3" expected_sha="$4"
  local owner_id mutation created create_error combined existing existing_id existing_sha sleep_for
  owner_id="$(organization_id "$organization")"
  mutation='mutation($name:String!,$ownerId:ID!,$description:String!,$visibility:RepositoryVisibility!){createRepository(input:{name:$name,ownerId:$ownerId,description:$description,visibility:$visibility,hasIssuesEnabled:true,hasWikiEnabled:false}){repository{id nameWithOwner url visibility isArchived isDisabled}}}'
  create_error="/tmp/den-2797-wave7-create-${organization}-${repository//\//-}.txt"

  for attempt in $(seq 1 40); do
    : > "$create_error"
    created=''
    if created="$(gh api graphql \
      -f query="$mutation" \
      -f name="$repository" \
      -f ownerId="$owner_id" \
      -f description="$description" \
      -f visibility=PUBLIC \
      2>"$create_error")"; then
      if ! jq -e '.errors and (.errors | length > 0)' >/dev/null 2>&1 <<<"$created"; then
        test "$(jq -r '.data.createRepository.repository.nameWithOwner' <<<"$created")" = "${organization}/${repository}"
        test "$(jq -r '.data.createRepository.repository.visibility' <<<"$created")" = PUBLIC
        echo "CREATED_PUBLIC ${organization}/${repository}"
        return 0
      fi
      jq -r '.errors[].message' <<<"$created" > "$create_error"
    fi

    combined="${created}$(cat "$create_error")"
    if grep -Eqi 'name already exists|already exists' <<<"$combined"; then
      existing="$(query_repository "$organization" "$repository")"
      existing_id="$(jq -r '.data.organization.repository.id // empty' <<<"$existing")"
      test -n "$existing_id"
      test "$(jq -r '.data.organization.repository.nameWithOwner' <<<"$existing")" = "${organization}/${repository}"
      test "$(jq -r '.data.organization.repository.visibility' <<<"$existing")" = PUBLIC
      test "$(jq -r '.data.organization.repository.isArchived' <<<"$existing")" = false
      test "$(jq -r '.data.organization.repository.isDisabled' <<<"$existing")" = false
      existing_sha="$(jq -r '.data.organization.repository.defaultBranchRef.target.oid // empty' <<<"$existing")"
      if [[ -n "$existing_sha" && "$existing_sha" != "$expected_sha" ]]; then
        echo "refusing divergent existing repository ${organization}/${repository}: ${existing_sha}" >&2
        return 1
      fi
      echo "PRESERVED_SAFE_EXISTING ${organization}/${repository} ${existing_sha:-empty}"
      return 0
    fi
    if grep -Eqi 'rate limit|secondary rate|abuse|temporarily unavailable|timeout' <<<"$combined"; then
      sleep_for=$(( attempt * 15 ))
      (( sleep_for <= 120 )) || sleep_for=120
      echo "Repository creation temporarily unavailable; retry ${attempt}/40 in ${sleep_for}s" >&2
      sleep "$sleep_for"
      continue
    fi
    printf '%s\n' "$combined" >&2
    return 1
  done
  echo "repository creation did not recover for ${organization}/${repository}" >&2
  return 1
}

push_or_verify() {
  local full_name="$1" directory="$2" expected_sha="$3"
  local organization="${full_name%%/*}" repository="${full_name#*/}"
  local existing existing_sha actual
  existing="$(query_repository "$organization" "$repository")"
  existing_sha="$(jq -r '.data.organization.repository.defaultBranchRef.target.oid // empty' <<<"$existing")"
  if [[ -n "$existing_sha" ]]; then
    test "$existing_sha" = "$expected_sha"
    echo "VERIFIED_EXISTING_MAIN $full_name $expected_sha"
    return 0
  fi
  if git -C "$directory" remote get-url origin >/dev/null 2>&1; then
    git -C "$directory" remote set-url origin "https://github.com/${full_name}.git"
  else
    git -C "$directory" remote add origin "https://github.com/${full_name}.git"
  fi
  GH_TOKEN="$GH_TOKEN" GIT_TERMINAL_PROMPT=0 \
    git -C "$directory" \
      -c credential.helper= \
      -c core.askPass="$askpass" \
      push --set-upstream origin main
  for _ in $(seq 1 30); do
    existing="$(query_repository "$organization" "$repository")"
    actual="$(jq -r '.data.organization.repository.defaultBranchRef.target.oid // empty' <<<"$existing")"
    [[ -z "$actual" ]] || break
    sleep 2
  done
  test "$actual" = "$expected_sha"
  echo "PUBLISHED_EXACT $full_name $expected_sha"
}

publish_one() {
  local full_name="$1" description="$2" directory="$3" expected_sha="$4"
  local organization="${full_name%%/*}" repository="${full_name#*/}"
  test "$(git -C "$directory" rev-parse HEAD)" = "$expected_sha"
  test "$(git -C "$directory" branch --show-current)" = main
  test -z "$(git -C "$directory" status --porcelain)"
  create_or_verify_empty "$organization" "$repository" "$description" "$expected_sha"
  push_or_verify "$full_name" "$directory" "$expected_sha"
}

publish_one \
  led-dynamo/leddy-sync \
  'Deterministic synchronization primitives for the Leddy fleet' \
  "$work/leddy-sync" \
  "$LEDDY_SYNC_HEAD"
publish_one \
  led-dynamo/leddy-mcp-server.rs \
  'Read-only MCP server for the Leddy fleet' \
  "$work/leddy-mcp-server.rs" \
  "$LEDDY_MCP_EXPECTED_SHA"
publish_one \
  canonical-cloud/canonical-docs \
  'Canonical Cloud architecture, operations, and business documentation' \
  "$work/canonical-docs" \
  "$CANONICAL_EXPECTED_SHA"
publish_one \
  evento-globolo/evgl-e2e \
  'Canonical Evento Globolo end-to-end and contract test harness' \
  "$work/evgl-e2e" \
  "$EVENTO_EXPECTED_SHA"
publish_one \
  hacker-house-medellin-test/hhm-e2e \
  'Hacker House Medellín test-organization end-to-end and contract harness' \
  "$work/hhm-e2e" \
  "$HHM_EXPECTED_SHA"

cat > /tmp/den-2797-wave7-evidence.json <<EOF
{
  "schema_version": 1,
  "published_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "control_repository": "$CONTROL_REPOSITORY",
  "control_workflow_sha": "$GITHUB_SHA",
  "force_push": false,
  "repositories": [
    {"name":"led-dynamo/leddy-sync","main":"$LEDDY_SYNC_HEAD","history":"exact-recovered"},
    {"name":"led-dynamo/leddy-mcp-server.rs","main":"$LEDDY_MCP_EXPECTED_SHA","recovered_head":"$LEDDY_MCP_HEAD","history":"exact-recovered-plus-formatting-fix"},
    {"name":"canonical-cloud/canonical-docs","main":"$CANONICAL_EXPECTED_SHA","history":"explicitly-reconstructed"},
    {"name":"evento-globolo/evgl-e2e","main":"$EVENTO_EXPECTED_SHA","history":"reviewed-seed"},
    {"name":"hacker-house-medellin-test/hhm-e2e","main":"$HHM_EXPECTED_SHA","history":"exact-recovered-plus-reviewed-overlay"}
  ],
  "excluded_ci_gated_targets": [
    "apostille-me/apme-e2e",
    "embedded-alerts/eal-e2e"
  ]
}
EOF
sed -i 's/^          //' /tmp/den-2797-wave7-evidence.json
jq -e . /tmp/den-2797-wave7-evidence.json >/dev/null
shred -u "$askpass"

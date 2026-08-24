#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly TARGET_REPOSITORY='ORESoftware/sustainable-bike'
readonly EXPECTED_BUNDLE_SHA256='ba05db2234aa7fb51d3784d3b725cca757b5d2a4b3399b4b60f8fc62982ab756'
readonly EXPECTED_BUNDLE_SIZE='240001'
readonly SLACK_FILE_ID='F0BS1HMNBNH'
readonly SLACK_CHANNEL_ID='C0BKP2N3LG7'
readonly SLACK_DOWNLOAD_URL='https://files.slack.com/files-pri/T01B3C83PMK-F0BS1HMNBNH/download/sustainable-bike-v0.2.0-complete.bundle'

: "${INSTANCE_ID:?AWS_SSM_INSTANCE_ID is required}"

remote_script="$RUNNER_TEMP/import-sustainable-bike-bundle.sh"
cat > "$remote_script" <<'REMOTE_EOF'
#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly TARGET_REPOSITORY='ORESoftware/sustainable-bike'
readonly EXPECTED_BUNDLE_SHA256='ba05db2234aa7fb51d3784d3b725cca757b5d2a4b3399b4b60f8fc62982ab756'
readonly EXPECTED_BUNDLE_SIZE='240001'
readonly SLACK_FILE_ID='F0BS1HMNBNH'
readonly SLACK_CHANNEL_ID='C0BKP2N3LG7'
readonly SLACK_DOWNLOAD_URL='https://files.slack.com/files-pri/T01B3C83PMK-F0BS1HMNBNH/download/sustainable-bike-v0.2.0-complete.bundle'
readonly BUNDLE_V01_ROOT='83da6ea14b7946b6947e3ba126b094c6f122f42d'
readonly BUNDLE_V01_FEATURE='5b8f5d5f02ac4b3e05b636cb505d632b426eda24'
readonly BUNDLE_V01_MAIN='3e99495cf7bf1a86b177fb747b23d9a15c578ac3'
readonly BUNDLE_POWER_HEAD='e2c1a0a3b4ae136265a58be1f1b331c046e1cf2c'
readonly BUNDLE_V02_MAIN='61e86ca632d7455aa91b6350579d0f8f39c5c0b5'
readonly BUNDLE_V01_TAG_OBJECT='4301d8c1e7447981ceb77244f1a12265141fb230'
readonly BUNDLE_V02_TAG_OBJECT='fb1e05076952ac856fef506b90f30093869f6759'

stage=initialization
work="$(mktemp -d /tmp/sustainable-bike-bundle-import.XXXXXX)"
GH_TOKEN=''
SLACK_TOKEN=''

cleanup() {
  status=$?
  set +e
  unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN SLACK_TOKEN
  unset encoded_pat encoded_slack GIT_ASKPASS GIT_ASKPASS_REQUIRE GIT_TERMINAL_PROMPT
  find "$work" -type f -exec sh -c 'for file do : > "$file"; done' sh {} + 2>/dev/null || true
  rm -rf "$work"
  exit "$status"
}
report_failure() {
  status=$?
  trap - ERR
  printf 'IMPORT_FAILED stage=%s status=%s\n' "$stage" "$status" >&2
  exit "$status"
}
trap cleanup EXIT
trap report_failure ERR

command -v kubectl >/dev/null
command -v gh >/dev/null
command -v git >/dev/null
command -v curl >/dev/null
command -v jq >/dev/null
test -r /etc/kubernetes/admin.conf

stage=protected-credentials
encoded_pat="$(
  KUBECONFIG=/etc/kubernetes/admin.conf \
    kubectl -n default get secret dd-agent-secrets \
    -o jsonpath='{.data.GH_PAT}'
)"
encoded_slack="$(
  KUBECONFIG=/etc/kubernetes/admin.conf \
    kubectl -n default get secret dd-slack-command-secrets \
    -o jsonpath='{.data.SLACK_BOT_TOKEN}'
)"
test -n "$encoded_pat"
test -n "$encoded_slack"
GH_TOKEN="$(printf '%s' "$encoded_pat" | base64 --decode)"
SLACK_TOKEN="$(printf '%s' "$encoded_slack" | base64 --decode)"
unset encoded_pat encoded_slack
test -n "$GH_TOKEN"
test -n "$SLACK_TOKEN"
[[ "$GH_TOKEN" != *$'\n'* && "$GH_TOKEN" != *$'\r'* ]]
[[ "$SLACK_TOKEN" != *$'\n'* && "$SLACK_TOKEN" != *$'\r'* ]]
export GH_TOKEN
export GITHUB_REPOSITORY_ADMIN_TOKEN="$GH_TOKEN"
test "$(gh api user --jq .login)" = ORESoftware
printf 'stage=%s status=passed\n' "$stage"

stage=download-and-verify-bundle
bundle="$work/sustainable-bike-v0.2.0.bundle"
join_response="$(
  curl --silent --show-error \
    --request POST \
    --header "Authorization: Bearer ${SLACK_TOKEN}" \
    --header 'Content-Type: application/x-www-form-urlencoded' \
    --data-urlencode "channel=${SLACK_CHANNEL_ID}" \
    https://slack.com/api/conversations.join \
    || true
)"
if [[ -n "$join_response" ]]; then
  jq -e '.ok == true or .error == "already_in_channel" or .error == "method_not_supported_for_channel_type" or .error == "missing_scope"' \
    <<<"$join_response" >/dev/null || true
fi

if ! curl --fail --location --silent --show-error \
  --retry 4 --retry-all-errors --retry-delay 2 \
  --header "Authorization: Bearer ${SLACK_TOKEN}" \
  "$SLACK_DOWNLOAD_URL" \
  --output "$bundle"; then
  file_info="$(
    curl --fail --silent --show-error \
      --header "Authorization: Bearer ${SLACK_TOKEN}" \
      --get \
      --data-urlencode "file=${SLACK_FILE_ID}" \
      https://slack.com/api/files.info
  )"
  jq -e '.ok == true and .file.id == "F0BS1HMNBNH"' <<<"$file_info" >/dev/null
  resolved_url="$(jq -er '.file.url_private_download | select(startswith("https://files.slack.com/"))' <<<"$file_info")"
  curl --fail --location --silent --show-error \
    --retry 4 --retry-all-errors --retry-delay 2 \
    --header "Authorization: Bearer ${SLACK_TOKEN}" \
    "$resolved_url" \
    --output "$bundle"
fi
test "$(wc -c < "$bundle" | tr -d ' ')" = "$EXPECTED_BUNDLE_SIZE"
test "$(sha256sum "$bundle" | awk '{print $1}')" = "$EXPECTED_BUNDLE_SHA256"
unset SLACK_TOKEN join_response file_info resolved_url
printf 'stage=%s status=passed sha256=%s size=%s\n' \
  "$stage" "$EXPECTED_BUNDLE_SHA256" "$EXPECTED_BUNDLE_SIZE"

stage=verify-bundle-objects
git init -q "$work/verifier"
git -C "$work/verifier" bundle verify "$bundle"
git clone -q "$bundle" "$work/source"
for object in \
  "$BUNDLE_V01_ROOT^{commit}" \
  "$BUNDLE_V01_FEATURE^{commit}" \
  "$BUNDLE_V01_MAIN^{commit}" \
  "$BUNDLE_POWER_HEAD^{commit}" \
  "$BUNDLE_V02_MAIN^{commit}" \
  "$BUNDLE_V01_TAG_OBJECT^{tag}" \
  "$BUNDLE_V02_TAG_OBJECT^{tag}"; do
  git -C "$work/source" cat-file -e "$object"
done
printf 'stage=%s status=passed commits=5 annotated_tags=2\n' "$stage"

stage=configure-git-authentication
askpass="$work/git-askpass.sh"
cat > "$askpass" <<'ASKPASS_EOF'
#!/usr/bin/env sh
case "${1:-}" in
  *Username*) printf '%s\n' x-access-token ;;
  *Password*) printf '%s\n' "${GH_TOKEN:?GH_TOKEN is required}" ;;
  *) exit 1 ;;
esac
ASKPASS_EOF
chmod 700 "$askpass"
export GIT_ASKPASS="$askpass"
export GIT_ASKPASS_REQUIRE=force
export GIT_TERMINAL_PROMPT=0
git -C "$work/source" remote add target "https://github.com/${TARGET_REPOSITORY}.git"
printf 'stage=%s status=passed\n' "$stage"

stage=publish-exact-bundle-refs
ensure_ref() {
  local source_object="$1"
  local destination="$2"
  local existing
  existing="$(git -C "$work/source" ls-remote target "$destination" | awk 'NR==1 {print $1}')"
  if [[ -n "$existing" ]]; then
    test "$existing" = "$source_object"
    printf 'VERIFIED_EXISTING ref=%s object=%s\n' "$destination" "$existing"
  else
    git -C "$work/source" -c credential.helper= push target "$source_object:$destination"
    printf 'PUBLISHED ref=%s object=%s\n' "$destination" "$source_object"
  fi
}

ensure_ref "$BUNDLE_V01_ROOT" refs/heads/history/bundle-v0.1.0-root
ensure_ref "$BUNDLE_V01_FEATURE" refs/heads/history/bundle-feat-parametric-captive-bike-v0
ensure_ref "$BUNDLE_V01_MAIN" refs/heads/history/bundle-v0.1.0-main
ensure_ref "$BUNDLE_POWER_HEAD" refs/heads/feat/power-and-export-hardening
ensure_ref "$BUNDLE_V02_MAIN" refs/heads/history/bundle-v0.2.0-main
ensure_ref "$BUNDLE_V01_TAG_OBJECT" refs/tags/v0.1.0
ensure_ref "$BUNDLE_V02_TAG_OBJECT" refs/tags/history/v0.2.0-local-bundle

existing_v02="$(git -C "$work/source" ls-remote target refs/tags/v0.2.0 | awk 'NR==1 {print $1}')"
test -n "$existing_v02"
test "$existing_v02" != "$BUNDLE_V02_TAG_OBJECT"
printf 'PRESERVED_EXISTING ref=refs/tags/v0.2.0 object=%s\n' "$existing_v02"
printf 'stage=%s status=passed\n' "$stage"

stage=attach-history-to-main
target="$work/target"
git clone -q "https://github.com/${TARGET_REPOSITORY}.git" "$target"
git -C "$target" config user.name 'ORESoftware publication automation'
git -C "$target" config user.email 'bot@oresoftware.dev'

attached=false
for attempt in 1 2 3; do
  git -C "$target" fetch -q origin main history/bundle-v0.2.0-main
  git -C "$target" checkout -q -B main origin/main
  current_tree="$(git -C "$target" rev-parse 'HEAD^{tree}')"
  if git -C "$target" merge-base --is-ancestor "$BUNDLE_V02_MAIN" HEAD; then
    attached=true
    break
  fi
  git -C "$target" merge \
    --no-ff \
    --allow-unrelated-histories \
    -s ours \
    -m 'merge(DEN-3843): attach original sustainable-bike bundle history' \
    origin/history/bundle-v0.2.0-main
  test "$(git -C "$target" rev-parse 'HEAD^{tree}')" = "$current_tree"
  if git -C "$target" -c credential.helper= push origin HEAD:refs/heads/main; then
    attached=true
    break
  fi
  sleep "$((attempt * 2))"
done
test "$attached" = true
git -C "$target" fetch -q origin main history/bundle-v0.2.0-main
final_main="$(git -C "$target" rev-parse origin/main)"
[[ "$final_main" =~ ^[0-9a-f]{40}$ ]]
git -C "$target" merge-base --is-ancestor "$BUNDLE_V02_MAIN" "$final_main"
printf 'stage=%s status=passed main=%s\n' "$stage" "$final_main"

stage=verify-remote-state
assert_remote_ref() {
  local expected="$1"
  local ref="$2"
  local observed
  observed="$(git -C "$work/source" ls-remote target "$ref" | awk 'NR==1 {print $1}')"
  test "$observed" = "$expected"
}
assert_remote_ref "$BUNDLE_V01_ROOT" refs/heads/history/bundle-v0.1.0-root
assert_remote_ref "$BUNDLE_V01_FEATURE" refs/heads/history/bundle-feat-parametric-captive-bike-v0
assert_remote_ref "$BUNDLE_V01_MAIN" refs/heads/history/bundle-v0.1.0-main
assert_remote_ref "$BUNDLE_POWER_HEAD" refs/heads/feat/power-and-export-hardening
assert_remote_ref "$BUNDLE_V02_MAIN" refs/heads/history/bundle-v0.2.0-main
assert_remote_ref "$BUNDLE_V01_TAG_OBJECT" refs/tags/v0.1.0
assert_remote_ref "$BUNDLE_V02_TAG_OBJECT" refs/tags/history/v0.2.0-local-bundle
test "$(git -C "$work/source" ls-remote target refs/heads/main | awk 'NR==1 {print $1}')" = "$final_main"

for commit in \
  "$BUNDLE_V01_ROOT" \
  "$BUNDLE_V01_FEATURE" \
  "$BUNDLE_V01_MAIN" \
  "$BUNDLE_POWER_HEAD" \
  "$BUNDLE_V02_MAIN"; do
  gh api "repos/${TARGET_REPOSITORY}/commits/${commit}" --jq .sha | grep -qx "$commit"
done
printf 'stage=%s status=passed exact_commits=5 exact_refs=7\n' "$stage"

stage=record-target-receipt
receipt="$(cat <<EOF
Complete local Git bundle imported without rebase or force-push.

- exact bundle SHA-256: \`${EXPECTED_BUNDLE_SHA256}\`
- original commits: \`${BUNDLE_V01_ROOT}\`, \`${BUNDLE_V01_FEATURE}\`, \`${BUNDLE_V01_MAIN}\`, \`${BUNDLE_POWER_HEAD}\`, \`${BUNDLE_V02_MAIN}\`
- exact power branch: \`feat/power-and-export-hardening@${BUNDLE_POWER_HEAD}\`
- exact annotated v0.1 tag object: \`${BUNDLE_V01_TAG_OBJECT}\`
- conflicting local v0.2 tag object retained at \`history/v0.2.0-local-bundle@${BUNDLE_V02_TAG_OBJECT}\`
- existing live \`v0.2.0\` was preserved at \`${existing_v02}\`
- newer live \`feat/parametric-captive-bike-v0\` was preserved; its original bundle head is at \`history/bundle-feat-parametric-captive-bike-v0\`
- final \`main\`: \`${final_main}\`; the five-commit bundle history is reachable as merged history

No credential value or Slack authorization token was persisted. Refs DEN-3843.
EOF
)"
gh pr comment 6 --repo "$TARGET_REPOSITORY" --body "$receipt" >/dev/null
printf 'IMPORT_SUCCESS repository=%s main=%s bundle_sha256=%s commits=5 refs=7\n' \
  "$TARGET_REPOSITORY" "$final_main" "$EXPECTED_BUNDLE_SHA256"
REMOTE_EOF
chmod 700 "$remote_script"

encoded_script="$(base64 --wrap=0 "$remote_script")"
remote_command="printf '%s' '$encoded_script' | base64 --decode > /tmp/import-sustainable-bike-bundle.sh; chmod 700 /tmp/import-sustainable-bike-bundle.sh; bash /tmp/import-sustainable-bike-bundle.sh > /tmp/import-sustainable-bike-bundle.log 2>&1; status=\$?; tail -c 20000 /tmp/import-sustainable-bike-bundle.log 2>/dev/null || true; rm -f /tmp/import-sustainable-bike-bundle.sh /tmp/import-sustainable-bike-bundle.log; exit \$status"
parameters="$RUNNER_TEMP/ssm-parameters.json"
jq -nc --arg command "$remote_command" '{commands:[$command]}' > "$parameters"

command_id="$(
  aws ssm send-command \
    --instance-ids "$INSTANCE_ID" \
    --document-name AWS-RunShellScript \
    --comment 'DEN-3843 import complete sustainable-bike bundle' \
    --parameters "file://$parameters" \
    --query 'Command.CommandId' \
    --output text
)"
[[ "$command_id" =~ ^[0-9a-f-]+$ ]]
echo "ssm_command_id=$command_id"

status=Pending
for _ in $(seq 1 500); do
  status="$(
    aws ssm get-command-invocation \
      --command-id "$command_id" \
      --instance-id "$INSTANCE_ID" \
      --query Status \
      --output text 2>/dev/null || true
  )"
  case "$status" in
    Success|Failed|Cancelled|TimedOut|Cancelling) break ;;
  esac
  sleep 3
done

invocation="$RUNNER_TEMP/ssm-invocation.json"
aws ssm get-command-invocation \
  --command-id "$command_id" \
  --instance-id "$INSTANCE_ID" \
  --query '{Status:Status,Stdout:StandardOutputContent,Stderr:StandardErrorContent}' \
  --output json > "$invocation"
jq . "$invocation"
test "$status" = Success
grep -q 'IMPORT_SUCCESS repository=ORESoftware/sustainable-bike' "$invocation"
final_main="$(
  jq -r '.Stdout | capture("IMPORT_SUCCESS repository=ORESoftware/sustainable-bike main=(?<sha>[0-9a-f]{40})").sha' \
    "$invocation"
)"
[[ "$final_main" =~ ^[0-9a-f]{40}$ ]]
printf 'AUTHORIZED_IMPORT_SUCCESS main=%s bundle_sha256=%s\n' \
  "$final_main" "$EXPECTED_BUNDLE_SHA256"

#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly ORG='meta-agents-demo'
readonly REPOSITORY='meta-agent-control-plane.rs'
readonly FULL_NAME="${ORG}/${REPOSITORY}"
readonly SOURCE_REPOSITORY='ORESoftware/k8s-cluster'
readonly SOURCE_SHA='55ee15c190b7cfa4e075f6984c7cb551acd4b9d3'
readonly BUNDLE_SHA256='1ddaa03743b864348162149b7d2d2e2dce7eab585cf092ea14547c647fcec031'
readonly PUBLISHER_SHA256='e2fe6eaa622db02a54f83e27a822f64ad4b54971c883f97bbda4ac0a4db5d278'
readonly EXPECTED_MAIN='4d6ec3ad0ec7b688f0e777129eee7e0f0d999df1'
readonly EXPECTED_FEATURE='789d48039da232faed985d4f8de176959f117e08'
readonly FEATURE_REF='agent/den-1057-meta-agent-control-plane'
readonly API_URL='https://api.github.com'
readonly API_VERSION='2022-11-28'

stage=bootstrap
work=''
report_failure() {
  local status=$?
  trap - ERR
  printf 'meta-agent-publisher-stage=%s status=failed exit=%s\n' "$stage" "$status" >&2
  exit "$status"
}
cleanup() {
  unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN installation_token app_jwt private_key
  if [[ -n "$work" ]]; then
    rm -rf "$work"
  fi
}
trap report_failure ERR
trap cleanup EXIT

: "${K8S_SUBMODULE_APP_ID:?K8S_SUBMODULE_APP_ID is required}"
: "${K8S_SUBMODULE_APP_PRIVATE_KEY:?K8S_SUBMODULE_APP_PRIVATE_KEY is required}"
[[ "$K8S_SUBMODULE_APP_ID" =~ ^[0-9]+$ ]]

work="$(mktemp -d /tmp/meta-agent-app-publisher.XXXXXX)"
private_key_file="$work/app-private-key.pem"
printf '%s' "$K8S_SUBMODULE_APP_PRIVATE_KEY" > "$private_key_file"
chmod 600 "$private_key_file"
grep -Eq '^-----BEGIN (RSA )?PRIVATE KEY-----$' "$private_key_file"

base64url() {
  openssl base64 -A | tr '+/' '-_' | tr -d '='
}

stage=mint-app-jwt
now="$(date +%s)"
iat="$((now - 60))"
exp="$((now + 540))"
header='{"alg":"RS256","typ":"JWT"}'
payload="$(printf '{"iat":%s,"exp":%s,"iss":%s}' "$iat" "$exp" "$K8S_SUBMODULE_APP_ID")"
unsigned="$(printf '%s' "$header" | base64url).$(printf '%s' "$payload" | base64url)"
signature="$(printf '%s' "$unsigned" | openssl dgst -sha256 -sign "$private_key_file" -binary | base64url)"
app_jwt="${unsigned}.${signature}"

stage=resolve-org-installation
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
test "$installation_status" = 200
installation_id="$(jq -er '.id | select(type == "number" and . > 0)' "$installation_json")"
repository_selection="$(jq -er '.repository_selection' "$installation_json")"
printf 'meta-agent-app-installation=%s repository_selection=%s\n' "$installation_id" "$repository_selection"

stage=mint-installation-token
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
test "$token_status" = 201
installation_token="$(jq -er '.token | select(type == "string" and length > 0)' "$token_json")"
admin_permission="$(jq -r '.permissions.administration // "none"' "$token_json")"
contents_permission="$(jq -r '.permissions.contents // "none"' "$token_json")"
metadata_permission="$(jq -r '.permissions.metadata // "none"' "$token_json")"
printf 'meta-agent-app-permissions administration=%s contents=%s metadata=%s\n' \
  "$admin_permission" "$contents_permission" "$metadata_permission"
test "$admin_permission" = write
test "$contents_permission" = write
if [[ -n "${GITHUB_ACTIONS:-}" ]]; then
  echo "::add-mask::${installation_token}"
fi
export GH_TOKEN="$installation_token"
export GITHUB_REPOSITORY_ADMIN_TOKEN="$installation_token"
unset app_jwt private_key K8S_SUBMODULE_APP_PRIVATE_KEY

stage=verify-installation-token
installation_repositories="$(gh api installation/repositories --paginate)"
current_count="$(jq '[.repositories[]? | select(.owner.login == "meta-agents-demo")] | length' <<<"$installation_repositories")"
printf 'meta-agent-visible-repositories-before=%s\n' "$current_count"

stage=checkout-exact-publisher-source
source_root="$work/source"
git init "$source_root"
git -C "$source_root" remote add origin "https://github.com/${SOURCE_REPOSITORY}.git"
git -C "$source_root" fetch --depth=1 origin "$SOURCE_SHA"
git -C "$source_root" checkout --detach FETCH_HEAD
test "$(git -C "$source_root" rev-parse HEAD)" = "$SOURCE_SHA"

stage=reconstruct-exact-bundle
bundle="$work/meta-agent-control-plane-den-1057.bundle"
cat "$source_root"/scripts/critical-org-fleet/assets/meta.part* | base64 --decode > "$bundle"
printf '%s  %s\n' "$BUNDLE_SHA256" "$bundle" | sha256sum --check --strict
test "$(git bundle list-heads "$bundle" | wc -l)" -eq 2

stage=validate-exact-publisher
publisher="$source_root/scripts/critical-org-fleet/publish_meta_control_plane.py"
printf '%s  %s\n' "$PUBLISHER_SHA256" "$publisher" | sha256sum --check --strict
python3 -m py_compile "$publisher"

stage=create-and-push-repository
python3 "$publisher" "$bundle"

stage=verify-live-repository
repo_json="$(gh api "repos/${FULL_NAME}")"
test "$(jq -r .owner.login <<<"$repo_json")" = "$ORG"
test "$(jq -r .visibility <<<"$repo_json")" = public
test "$(jq -r .default_branch <<<"$repo_json")" = main
main_sha="$(gh api "repos/${FULL_NAME}/git/ref/heads/main" --jq .object.sha)"
feature_sha="$(gh api "repos/${FULL_NAME}/git/ref/heads/${FEATURE_REF}" --jq .object.sha)"
test "$main_sha" = "$EXPECTED_MAIN"
test "$feature_sha" = "$EXPECTED_FEATURE"

stage=complete
printf 'META_AGENT_REPOSITORY=%s\n' "$FULL_NAME"
printf 'META_AGENT_MAIN=%s\n' "$main_sha"
printf 'META_AGENT_FEATURE_REF=%s\n' "$FEATURE_REF"
printf 'META_AGENT_FEATURE_SHA=%s\n' "$feature_sha"
printf 'META_AGENT_VISIBILITY=public\n'
printf 'META_AGENT_DEFAULT_BRANCH=main\n'

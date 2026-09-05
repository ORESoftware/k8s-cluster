#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly trusted_sha="${1:?trusted k8s-cluster SHA required}"
readonly evidence_name='repository-publication.json'
readonly evidence_prefix='ORES_MANIFESTOS_EVIDENCE_BASE64='

[[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]]
command -v git >/dev/null 2>&1
command -v jq >/dev/null 2>&1
command -v python3 >/dev/null 2>&1
command -v base64 >/dev/null 2>&1

repo_root="$(git rev-parse --show-toplevel 2>/dev/null)"
test -d "$repo_root/.git"
test "$(git -C "$repo_root" rev-parse HEAD)" = "$trusted_sha"

protected_token="${PROTECTED_GITHUB_TOKEN:-}"
test -n "$protected_token"
[[ "$protected_token" != *$'\n'* ]]
[[ "$protected_token" != *$'\r'* ]]
[[ "$protected_token" != *$'\t'* ]]
[[ "$protected_token" != *' '* ]]

work="$(mktemp -d /tmp/ores-manifestos-protected-publisher.XXXXXX)"
cleanup() {
  unset protected_token PROTECTED_GITHUB_TOKEN GH_TOKEN GITHUB_TOKEN
  unset GITHUB_REPOSITORY_ADMIN_TOKEN evidence_payload
  rm -rf "$work"
}
trap cleanup EXIT INT TERM

export GH_TOKEN="$protected_token"
export GITHUB_REPOSITORY_ADMIN_TOKEN="$protected_token"
unset PROTECTED_GITHUB_TOKEN protected_token

evidence_path="$work/$evidence_name"
python3 "$repo_root/scripts/ops/create_ores_manifestos_repositories_20260905.py" \
  --evidence "$evidence_path"

normalized_path="$work/repository-publication.normalized.json"
jq '.credential_source = "protected-ssm-host"' "$evidence_path" > "$normalized_path"
mv "$normalized_path" "$evidence_path"
chmod 600 "$evidence_path"

jq -e '
  .schema_version == 1 and
  .organization == "ores-manifestos" and
  .actor == "ORESoftware" and
  .repository_count == 2 and
  .pages.build_type == "workflow" and
  .pages.html_url == "https://ores-manifestos.github.io/" and
  .credential_source == "protected-ssm-host" and
  (.credential_persisted == false) and
  (.credential_revoked == false) and
  (([.repositories[].full_name] | sort) == ([
    "ores-manifestos/ores-manifestos-docs",
    "ores-manifestos/ores-manifestos.github.io"
  ] | sort)) and
  (all(.repositories[];
    .visibility == "public" and
    .default_branch == "main" and
    (.repository_id | type == "number" and . > 0) and
    (.main_sha | test("^[0-9a-f]{40}$"))))
' "$evidence_path" >/dev/null

evidence_payload="$(base64 --wrap=0 "$evidence_path")"
test -n "$evidence_payload"
printf '%s%s\n' "$evidence_prefix" "$evidence_payload"
printf 'ORES_MANIFESTOS_PROTECTED_PUBLISHER_READY trusted_sha=%s\n' "$trusted_sha"

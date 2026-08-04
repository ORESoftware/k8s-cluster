#!/usr/bin/env bash
set -Eeuo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
workflow="$repo_root/.github/workflows/ops-shared-auth-introspection-exact-head.yml"
runbook="$repo_root/docs/operations/shared-auth-introspection-exact-head-validation.md"

fail() {
  printf 'shared-auth validator contract: %s\n' "$*" >&2
  exit 1
}

require_literal() {
  local needle="$1"
  local file="${2:-$workflow}"
  grep -Fq -- "$needle" "$file" || fail "missing literal in ${file#"$repo_root/"}: $needle"
}

require_regex() {
  local expression="$1"
  local file="${2:-$workflow}"
  grep -Eq -- "$expression" "$file" || fail "missing pattern in ${file#"$repo_root/"}: $expression"
}

[[ -f "$workflow" ]] || fail "workflow is missing"
[[ -f "$runbook" ]] || fail "runbook is missing"

require_literal 'pull_request_target:'
require_literal 'paths:'
require_literal '.github/shared-auth-introspection-validation-trigger'
require_literal "github.event.pull_request.draft == true"
require_literal "github.event.pull_request.user.login == 'ORESoftware'"
require_literal "github.event.pull_request.head.repo.full_name == github.repository"
require_literal "startsWith(github.event.pull_request.head.ref, 'agent/shared-auth-introspection-validation-')"
require_literal "github.event.pull_request.title == 'DO NOT MERGE: validate Shared Auth introspection exact head'"
require_literal 'TARGET_REPOSITORY: shared-auth/shared-auth-server.rs'
require_literal "TARGET_PR: '30'"
require_literal 'TARGET_SHA: 4148e5b96a448a20da00922cc62386455e211126'
require_literal 'TARGET_BRANCH: agent/shared-auth-auth-time-claim'

require_literal '.commits == 1'
require_literal '.changed_files == 1'
require_literal '.additions == 6'
require_literal '.deletions == 0'
require_literal '.head.repo.full_name == "ORESoftware/k8s-cluster"'
require_literal '.title == "DO NOT MERGE: validate Shared Auth introspection exact head"'
require_literal 'test "$(sed -n '\''s/^protocol=//p'\'' <<<"$marker")" = rsa-oaep-sha256-v1'
require_literal 'test "$(sed -n '\''s/^purpose=//p'\'' <<<"$marker")" = shared-auth-introspection-validation'
require_literal "jq -e '(.status == \"ahead\" or .status == \"identical\") and .behind_by == 0'"

require_literal 'openssl genpkey -quiet -algorithm RSA -pkeyopt rsa_keygen_bits:3072'
require_literal '-pkeyopt rsa_padding_mode:oaep'
require_literal '-pkeyopt rsa_oaep_md:sha256'
require_literal '-pkeyopt rsa_mgf1_md:sha256'
require_literal 'select(.user.login == "ORESoftware")'
require_literal 'select(.id > $challenge_id)'
require_literal 'for _ in $(seq 1 180)'
require_literal '[[ "$owner_token" == ghp_* || "$owner_token" == github_pat_* ]]'
require_literal 'test "$(GH_TOKEN="$owner_token" gh api user --jq '\''.login'\'' 2>/dev/null)" = ORESoftware'
require_literal '.state == "open" and .head.sha == $sha and .head.ref == $branch and .base.ref == "main"'
require_literal 'GH_TOKEN="$owner_token" gh api "repos/${TARGET_REPOSITORY}" --jq '\''.permissions.pull == true'\'' | grep -qx true'

require_literal 'export GIT_TERMINAL_PROMPT=0'
require_literal 'export GIT_ASKPASS="$askpass"'
require_literal '-c protocol.ext.allow=never -c protocol.file.allow=never fetch --depth 1 --no-tags origin "$TARGET_SHA"'
require_literal 'git -C "$source_root" checkout -q --detach FETCH_HEAD'
require_literal 'git -C "$source_root" remote remove origin'
require_literal 'unset VALIDATION_OWNER_TOKEN GH_TOKEN GITHUB_TOKEN GIT_ASKPASS'
require_literal 'rm -f "$askpass" "$private_key" "$public_key" "$ciphertext_file"'
require_literal 'rm -rf "$work"'

unset_line="$(grep -nF 'unset VALIDATION_OWNER_TOKEN GH_TOKEN GITHUB_TOKEN GIT_ASKPASS' "$workflow" | cut -d: -f1)"
remove_line="$(grep -nF 'rm -f "$askpass" "$private_key" "$public_key" "$ciphertext_file"' "$workflow" | cut -d: -f1)"
execute_line="$(grep -nF 'cd "$source_root"' "$workflow" | cut -d: -f1)"
first_cargo_line="$(grep -nE '^          cargo (fmt|clippy|test|build)' "$workflow" | head -n1 | cut -d: -f1)"
[[ "$unset_line" =~ ^[0-9]+$ && "$remove_line" =~ ^[0-9]+$ && "$execute_line" =~ ^[0-9]+$ && "$first_cargo_line" =~ ^[0-9]+$ ]] || fail "could not locate credential boundary ordering"
(( unset_line < execute_line && remove_line < execute_line && execute_line < first_cargo_line )) || fail "repository code can execute before credential destruction"

if grep -Eq '^[[:space:]]*-[[:space:]]*uses:[[:space:]]*actions/checkout@' "$workflow"; then
  fail "trusted workflow must not checkout carrier-controlled content"
fi
if grep -Fq 'actions/upload-artifact@' "$workflow"; then
  fail "trusted workflow must not upload private source or logs as artifacts"
fi
if grep -Eq 'github\.event\.pull_request\.head\.(sha|ref).*(checkout|fetch)' "$workflow"; then
  fail "trusted workflow references an untrusted carrier head for execution"
fi

while IFS= read -r entry; do
  ref="${entry#*uses: }"
  ref="${ref%% *}"
  [[ "$ref" =~ ^(\./|\.\./) ]] && continue
  [[ "$ref" =~ @[0-9a-f]{40}$ ]] || fail "mutable action reference: $ref"
done < <(grep -E '^[[:space:]]*-[[:space:]]*uses:' "$workflow" || true)

require_literal 'cargo fmt --all --check'
require_literal 'psql "$AUTH_TEST_DATABASE_URL" -v ON_ERROR_STOP=1 -f db/schema.sql'
require_literal 'cargo clippy --all-targets --locked -- -D warnings'
require_literal 'cargo test --all-targets --locked'
require_literal 'cargo test --locked http::introspect::tests::'
require_literal 'cargo test --locked --test integration introspect_fails_closed_when_secret_unset -- --exact'
require_literal '(cd e2e && npm test)'
require_literal 'docker build -t shared-auth-server:exact-head-validation .'
require_literal "gh pr close \"$PR_NUMBER\" --repo \"$REPOSITORY\" --comment 'Exact-head validation completed; this metadata-only carrier must not be merged.'"

require_literal '`4148e5b96a448a20da00922cc62386455e211126`' "$runbook"
require_literal 'destroyed before repository code executed' "$runbook"
require_literal 'duplicate-header' "$runbook"

printf 'Shared Auth exact-head validator trust contract passed.\n'

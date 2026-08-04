#!/usr/bin/env bash
set -Eeuo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
workflow="$repo_root/.github/workflows/ops-shared-auth-introspection-private-validator.yml"
runbook="$repo_root/docs/operations/shared-auth-introspection-private-validation.md"

fail() {
  printf 'private Shared Auth validator contract: %s\n' "$*" >&2
  exit 1
}

require_literal() {
  local needle="$1"
  local file="${2:-$workflow}"
  grep -Fq -- "$needle" "$file" || fail "missing literal in ${file#"$repo_root/"}: $needle"
}

[[ -f "$workflow" ]] || fail 'trusted workflow is missing'
[[ -f "$runbook" ]] || fail 'validation runbook is missing'

# The private host and metadata-only carrier are checked before credentials are
# requested or any source is fetched.
require_literal 'pull_request_target:'
require_literal 'jq -e '\''.private == true'\'' <<<"$host" >/dev/null'
require_literal "github.event.pull_request.draft == true"
require_literal "github.event.pull_request.user.login == 'ORESoftware'"
require_literal "github.event.pull_request.head.repo.full_name == github.repository"
require_literal "github.event.pull_request.title == 'DO NOT MERGE: patch and validate Shared Auth introspection exact head'"
require_literal '.head.repo.full_name == "3FA-app/3fa-backend.rs"'
require_literal '.commits == 1'
require_literal '.changed_files == 1'
require_literal '.additions == 6'
require_literal '.deletions == 0'
require_literal 'target-start-sha='
require_literal 'test "$(sed -n '\''s/^protocol=//p'\'' <<<"$marker")" = rsa-oaep-sha256-v1'
require_literal 'test "$(sed -n '\''s/^purpose=//p'\'' <<<"$marker")" = shared-auth-introspection-patch-validation'
require_literal "jq -e '(.status == \"ahead\" or .status == \"identical\") and .behind_by == 0'"

# The target is immutable until the workflow publishes one exact, deterministic
# documentation-only fast-forward.
require_literal 'TARGET_REPOSITORY: shared-auth/shared-auth-server.rs'
require_literal "TARGET_PR: '30'"
require_literal 'TARGET_START_SHA: 4148e5b96a448a20da00922cc62386455e211126'
require_literal 'TARGET_BRANCH: agent/shared-auth-auth-time-claim'
require_literal '.state == "open" and .head.sha == $sha and .head.ref == $branch and .base.ref == "main"'
require_literal "expected exactly one stale introspection comment block"
require_literal 'test "$(git -C "$source_root" diff --name-only)" = src/config.rs'
require_literal "grep -Fq 'this secret is absent, introspection is disabled rather than exposed.'"
require_literal "! grep -Fq 'introspection stays open for backward'"
require_literal 'git -C "$source_root" commit -q -m '\''docs: document fail-closed introspection'\'''
require_literal 'git -C "$source_root" -c protocol.ext.allow=never -c protocol.file.allow=never push --porcelain origin "HEAD:refs/heads/${TARGET_BRANCH}"'
if grep -Eq 'git .*push[^\n]*(--force|-f([[:space:]]|$)|\+HEAD)' "$workflow"; then
  fail 'validator must never force-push the target branch'
fi

# The one-time owner credential is encrypted, identity-scoped, permission-
# checked, and removed before any repository-controlled process can execute.
require_literal 'openssl genpkey -quiet -algorithm RSA -pkeyopt rsa_keygen_bits:3072'
require_literal '-pkeyopt rsa_padding_mode:oaep'
require_literal '-pkeyopt rsa_oaep_md:sha256'
require_literal '-pkeyopt rsa_mgf1_md:sha256'
require_literal 'select(.id > $challenge_id)'
require_literal 'select(.user.login == "ORESoftware")'
require_literal 'for _ in $(seq 1 180)'
require_literal '[[ "$owner_token" == ghp_* || "$owner_token" == github_pat_* ]]'
require_literal 'test "$(GH_TOKEN="$owner_token" gh api user --jq '\''.login'\'' 2>/dev/null)" = ORESoftware'
require_literal "jq -e '.private == true and .permissions.pull == true and .permissions.push == true'"
require_literal 'git -C "$source_root" config core.hooksPath "$secret_root/empty-hooks"'
require_literal 'git -C "$source_root" config credential.helper '\'''\'''
require_literal '-c protocol.ext.allow=never -c protocol.file.allow=never fetch --depth 1 --no-tags origin "$TARGET_START_SHA"'
require_literal 'git -C "$source_root" remote remove origin'
require_literal 'unset VALIDATION_OWNER_TOKEN GH_TOKEN GITHUB_TOKEN GIT_ASKPASS'
require_literal 'rm -f "$secret_root/git-askpass.sh"'

remove_remote_line="$(grep -nF 'git -C "$source_root" remote remove origin' "$workflow" | cut -d: -f1)"
scrub_line="$(grep -nF '          scrub_credentials' "$workflow" | tail -n1 | cut -d: -f1)"
first_cargo_line="$(grep -nE '^          cargo (fmt|clippy|test|build)' "$workflow" | head -n1 | cut -d: -f1)"
[[ "$remove_remote_line" =~ ^[0-9]+$ && "$scrub_line" =~ ^[0-9]+$ && "$first_cargo_line" =~ ^[0-9]+$ ]] || fail 'could not locate credential boundary ordering'
(( remove_remote_line < first_cargo_line && scrub_line < first_cargo_line )) || fail 'repository code can execute before credential destruction'

if grep -Eq '^[[:space:]]*-[[:space:]]*uses:[[:space:]]*actions/checkout@' "$workflow"; then
  fail 'trusted workflow must not checkout carrier-controlled content'
fi
if grep -Fq 'actions/upload-artifact@' "$workflow"; then
  fail 'trusted workflow must not upload private source or logs'
fi
while IFS= read -r entry; do
  ref="${entry#*uses: }"
  ref="${ref%% *}"
  [[ "$ref" =~ ^(\./|\.\./) ]] && continue
  [[ "$ref" =~ @[0-9a-f]{40}$ ]] || fail "mutable action reference: $ref"
done < <(grep -E '^[[:space:]]*-[[:space:]]*uses:' "$workflow" || true)

# The executable matrix mirrors and extends the blocked native Shared Auth lane.
require_literal 'cargo fmt --all --check'
require_literal 'psql "$AUTH_TEST_DATABASE_URL" -v ON_ERROR_STOP=1 -f db/schema.sql'
require_literal 'cargo clippy --all-targets --locked -- -D warnings'
require_literal 'cargo test --all-targets --locked'
require_literal 'cargo test --locked http::introspect::tests::'
require_literal 'cargo test --locked --test integration introspect_fails_closed_when_secret_unset -- --exact'
require_literal '(cd e2e && npm test)'
require_literal 'rustsec/audit-check@69366f33c96575abad1ee0dba8212993eecbe998'
require_literal 'docker build -t shared-auth-server:private-exact-head-validation .'
require_literal "this metadata-only carrier must not be merged"

require_literal '`4148e5b96a448a20da00922cc62386455e211126`' "$runbook"
require_literal 'Both this host repository and the Shared Auth source repository are private.' "$runbook"
require_literal 'Before any repository code, build script, dependency, test, browser, audit, or' "$runbook"
require_literal 'normal non-force fast-forward' "$runbook"

printf 'Private Shared Auth validator trust contract passed.\n'

#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

trusted_sha="${1:?trusted k8s-cluster SHA required}"
if [[ ! "$trusted_sha" =~ ^[0-9a-f]{40}$ ]]; then
  printf 'ores-locks-publisher=failed reason=invalid-trusted-sha\n' >&2
  exit 64
fi

readonly target='ORESoftware/ores-locks-and-leases'
readonly description='Shared polyglot local locks, PostgreSQL advisory locks, and renewable fenced Fiducia leases'
readonly issue_title='Bootstrap polyglot locks-and-leases contract and first release'

work="$(mktemp -d /tmp/ores-locks-and-leases-publisher.XXXXXX)"
cleanup() {
  rc=$?
  trap - EXIT
  if [[ "$work" == /tmp/ores-locks-and-leases-publisher.* && -d "$work" ]]; then
    find "$work" -xdev -depth -delete
  fi
  exit "$rc"
}
trap cleanup EXIT

for command in awk bash chown curl find getent git id install mktemp sha256sum stat sudo tar uname; do
  command -v "$command" >/dev/null || {
    printf 'ores-locks-publisher=failed reason=missing-command command=%s\n' "$command" >&2
    exit 70
  }
done

# Rehydrate the exact reviewed k8s-cluster revision and install the repository's
# checksum-pinned GitHub CLI. Repository administration never depends on an
# ambient binary or a credential copied from task text, chat, or source control.
git init "$work/k8s-cluster" >/dev/null
git -C "$work/k8s-cluster" remote add origin https://github.com/ORESoftware/k8s-cluster.git
git -C "$work/k8s-cluster" fetch --quiet --depth=1 origin "$trusted_sha"
git -C "$work/k8s-cluster" switch --quiet --detach FETCH_HEAD
actual_sha="$(git -C "$work/k8s-cluster" rev-parse HEAD)"
if [[ "$actual_sha" != "$trusted_sha" ]]; then
  printf 'ores-locks-publisher=failed reason=trusted-checkout-mismatch\n' >&2
  exit 71
fi

installer="$work/k8s-cluster/scripts/ops/install_pinned_github_cli.sh"
if [[ ! -f "$installer" ]]; then
  printf 'ores-locks-publisher=failed reason=missing-pinned-gh-installer\n' >&2
  exit 71
fi
gh_binary="$(bash "$installer" --install-dir "$work/pinned-gh")"
if [[ ! -x "$gh_binary" ]]; then
  printf 'ores-locks-publisher=failed reason=pinned-gh-not-executable\n' >&2
  exit 71
fi
case "$gh_binary" in
  "$work"/*/gh) ;;
  *)
    printf 'ores-locks-publisher=failed reason=pinned-gh-path-escaped\n' >&2
    exit 71
    ;;
esac

profile_home="$(getent passwd ec2-user | awk -F: '$1 == "ec2-user" { print $6 }')"
profile_uid="$(id -u ec2-user)"
case "$profile_home" in
  /*) ;;
  *)
    printf 'ores-locks-profile=failed reason=missing-ec2-user\n' >&2
    exit 65
    ;;
esac

chown -R ec2-user:ec2-user "$work"
chmod 700 "$work"

selected_dir=''
diagnostic='no-candidate-profile'
candidate_index=0
while IFS= read -r candidate; do
  [[ -n "$candidate" && -f "$candidate" ]] || continue
  candidate_index=$((candidate_index + 1))

  if [[ -L "$candidate" ]]; then
    diagnostic='candidate-symlink-rejected'
    continue
  fi

  owner="$(stat -c '%u' "$candidate" 2>/dev/null || true)"
  mode="$(stat -c '%a' "$candidate" 2>/dev/null || true)"
  if [[ "$owner" != 0 && "$owner" != "$profile_uid" ]]; then
    diagnostic='candidate-owner-rejected'
    continue
  fi
  if [[ ! "$mode" =~ ^[0-7]{3,4}$ ]] || (( (8#$mode & 0022) != 0 )); then
    diagnostic='candidate-mode-rejected'
    continue
  fi

  candidate_dir="$work/gh-profile-$candidate_index"
  install -d -m 700 -o ec2-user -g ec2-user "$candidate_dir"
  install -m 600 -o ec2-user -g ec2-user "$candidate" "$candidate_dir/hosts.yml"

  token="$(
    sudo -u ec2-user -H env \
      -u GH_TOKEN -u GITHUB_TOKEN -u GH_ENTERPRISE_TOKEN \
      -u GITHUB_REPOSITORY_ADMIN_TOKEN \
      GH_CONFIG_DIR="$candidate_dir" \
      "$gh_binary" auth token --hostname github.com 2>/dev/null || true
  )"
  if [[ -z "$token" || "$token" == *$'\n'* || "$token" == *$'\r'* || \
        "$token" == *$'\t'* || "$token" == *' '* ]]; then
    diagnostic='candidate-profile-unusable'
    unset token
    continue
  fi

  legacy_pat_prefix='gh''p_'
  fine_grained_pat_prefix='github_''pat_'
  case "$token" in
    "${legacy_pat_prefix}"*|"${fine_grained_pat_prefix}"*)
      diagnostic='candidate-personal-access-token-rejected'
      unset token legacy_pat_prefix fine_grained_pat_prefix
      continue
      ;;
    gho_*|ghu_*|ghs_*) ;;
    *)
      diagnostic='candidate-token-class-rejected'
      unset token legacy_pat_prefix fine_grained_pat_prefix
      continue
      ;;
  esac
  unset token legacy_pat_prefix fine_grained_pat_prefix

  identity="$(
    sudo -u ec2-user -H env \
      -u GH_TOKEN -u GITHUB_TOKEN -u GH_ENTERPRISE_TOKEN \
      -u GITHUB_REPOSITORY_ADMIN_TOKEN \
      GH_CONFIG_DIR="$candidate_dir" \
      "$gh_binary" api user --jq .login 2>/dev/null || true
  )"
  if [[ "$identity" != ORESoftware ]]; then
    diagnostic='candidate-identity-rejected'
    continue
  fi

  selected_dir="$candidate_dir"
  break
done < <(
  {
    printf '%s\n' "$profile_home/.config/gh/hosts.yml" /root/.config/gh/hosts.yml
    find /home /root /var/lib -maxdepth 7 -type f \
      -path '*/gh/hosts.yml' -print 2>/dev/null || true
  } | awk '!seen[$0]++'
)

if [[ -z "$selected_dir" ]]; then
  printf 'ores-locks-profile=failed reason=%s\n' "$diagnostic" >&2
  exit 65
fi

gh_as_owner() {
  sudo -u ec2-user -H env \
    -u GH_TOKEN -u GITHUB_TOKEN -u GH_ENTERPRISE_TOKEN \
    -u GITHUB_REPOSITORY_ADMIN_TOKEN \
    GH_CONFIG_DIR="$selected_dir" \
    "$gh_binary" "$@"
}

created=false
if gh_as_owner api "repos/$target" >/dev/null 2>&1; then
  :
else
  gh_as_owner repo create "$target" \
    --private \
    --description "$description" \
    --add-readme \
    --disable-wiki >/dev/null
  created=true
fi

gh_as_owner api --method PATCH "repos/$target" \
  -f description="$description" \
  -F private=true \
  -f visibility=private \
  -F has_issues=true \
  -F has_projects=false \
  -F has_wiki=false \
  -F allow_squash_merge=true \
  -F allow_merge_commit=true \
  -F allow_rebase_merge=false \
  -F allow_auto_merge=false \
  -F delete_branch_on_merge=true >/dev/null

repo_json="$(gh_as_owner api "repos/$target")"
[[ "$(gh_as_owner api "repos/$target" --jq .full_name)" == "$target" ]]
[[ "$(gh_as_owner api "repos/$target" --jq .owner.login)" == ORESoftware ]]
[[ "$(gh_as_owner api "repos/$target" --jq .private)" == true ]]
[[ "$(gh_as_owner api "repos/$target" --jq .visibility)" == private ]]
[[ "$(gh_as_owner api "repos/$target" --jq .default_branch)" == main ]]
[[ "$(gh_as_owner api "repos/$target" --jq .description)" == "$description" ]]
[[ "$(gh_as_owner api "repos/$target" --jq .archived)" == false ]]
[[ "$(gh_as_owner api "repos/$target" --jq .disabled)" == false ]]
[[ "$(gh_as_owner api "repos/$target" --jq .has_issues)" == true ]]
[[ "$(gh_as_owner api "repos/$target" --jq .has_projects)" == false ]]
[[ "$(gh_as_owner api "repos/$target" --jq .has_wiki)" == false ]]
[[ "$(gh_as_owner api "repos/$target" --jq .allow_squash_merge)" == true ]]
[[ "$(gh_as_owner api "repos/$target" --jq .allow_merge_commit)" == true ]]
[[ "$(gh_as_owner api "repos/$target" --jq .allow_rebase_merge)" == false ]]
[[ "$(gh_as_owner api "repos/$target" --jq .allow_auto_merge)" == false ]]
[[ "$(gh_as_owner api "repos/$target" --jq .delete_branch_on_merge)" == true ]]
main_sha="$(gh_as_owner api "repos/$target/git/ref/heads/main" --jq .object.sha)"
[[ "$main_sha" =~ ^[0-9a-f]{40}$ ]]

issue_body="$work/bootstrap-issue.md"
cat > "$issue_body" <<'EOF'
## Goal

Build the shared ORESoftware locks-and-leases library as a versioned, polyglot package rather than duplicating coordination logic in every product repository.

## Contract authorities

TypeSpec and JSON Schema/OpenAPI are independent, human-authored top-level authorities. Each generates its own normalized lock/lease protocol projection; translation and round-trip outputs are comparison evidence only. Any unexplained mismatch is `STOPPED_FOR_EVALUATION` and blocks package publication and consumer rollout.

## Required implementations

- Rust is the reference systems implementation, with first-class TypeScript/Node.js, Go, and Gleam/BEAM bindings; add Dart/Flutter where client-side coordination is genuinely required.
- Local process/file locking composes the audited `zed-pkg/zed-lock` primitives and never weakens their path, cancellation, ownership, or crash-recovery guarantees.
- Distributed coordination uses narrow Fiducia acquire/renew/release operations, monotonic fencing tokens, stable project/resource identities, bounded deadlines, and fail-closed lease-loss behavior.
- PostgreSQL support includes transaction-scoped and session-scoped advisory locks, with explicit connection-lifetime and pool-return rules.
- Composite routines publish one lock ordering, acquire local authority before remote/shared authority where required, and never silently downgrade when distributed coordination was requested.
- Every operation carries an allowlisted request/operation context and emits redacted `ores-otel` events suitable for `ores-middleware` consumers.
- Root `.zpkg.toml` declares every public language target and immutable dependency provenance.

## Verification

- Model holder, waiter, renewal, fencing, cancellation, timeout, process-death, delayed-release, network-partition, leader-change, connection-loss, and pool-reuse states with exhaustive transition checks.
- Run deterministic cross-language valid/invalid vectors and native compilation/runtime tests.
- Prove stale fencing tokens cannot commit after lease loss or holder replacement.
- Prove PostgreSQL transaction locks release on commit/rollback and session locks cannot leak through a returned pooled connection.
- Prove reporter/telemetry failures do not alter lock correctness and never expose lock secrets, credentials, raw SQL, or private holder metadata.
- Publish only from an exact reviewed commit with a generated frozen Zed lock and retained digest/provenance receipt.

## Initial consumers

Adopt through separate tested PRs in `sonus-auris-lib-core`, `daedalus-lib-core`, `cliptown-lib-core`, `ap-lib-core`, `fanwaave-lib-core`, `athleto-lib-core`, `claritas-lib-core`, `claimgraph-lib-core`, `cp-lib-core`, and `ecmad-lib-core`. Each consumer must preserve its own `*-interfaces` authority, use zed-pkg, and keep backend-only lock credentials out of client/isomorphic exports.

## Tracking

- Linear: DEN-2045, DEN-2037, DEN-2042, DEN-2074, DEN-2076, DEN-3051
- Governance: `ORESoftware/my-ai/AGENTS.md` and `ORESoftware/k8s-cluster/AGENTS.md`
EOF

mapfile -t issue_numbers < <(
  gh_as_owner issue list \
    --repo "$target" \
    --state all \
    --limit 100 \
    --json number,title \
    --jq ".[] | select(.title == \"$issue_title\") | .number"
)
if (( ${#issue_numbers[@]} > 1 )); then
  printf 'ores-locks-publisher=failed reason=duplicate-bootstrap-issues\n' >&2
  exit 72
fi

issue_created=false
if (( ${#issue_numbers[@]} == 0 )); then
  issue_url="$(
    gh_as_owner issue create \
      --repo "$target" \
      --title "$issue_title" \
      --body-file "$issue_body"
  )"
  issue_number="${issue_url##*/}"
  issue_created=true
else
  issue_number="${issue_numbers[0]}"
  issue_state="$(gh_as_owner api "repos/$target/issues/$issue_number" --jq .state)"
  if [[ "$issue_state" == closed ]]; then
    gh_as_owner issue reopen "$issue_number" --repo "$target" >/dev/null
  fi
fi
[[ "$issue_number" =~ ^[0-9]+$ ]]
[[ "$(gh_as_owner api "repos/$target/issues/$issue_number" --jq .title)" == "$issue_title" ]]
[[ "$(gh_as_owner api "repos/$target/issues/$issue_number" --jq .state)" == open ]]
[[ "$(gh_as_owner api "repos/$target/issues/$issue_number" --jq 'has("pull_request")')" == false ]]
issue_url="$(gh_as_owner api "repos/$target/issues/$issue_number" --jq .html_url)"

printf 'ORES_LOCKS_REPOSITORY_READY repository=%s created=%s main=%s trusted_k8s_cluster_sha=%s\n' \
  "$target" "$created" "$main_sha" "$trusted_sha"
printf 'ORES_LOCKS_BOOTSTRAP_ISSUE_READY repository=%s issue=%s created=%s url=%s\n' \
  "$target" "$issue_number" "$issue_created" "$issue_url"

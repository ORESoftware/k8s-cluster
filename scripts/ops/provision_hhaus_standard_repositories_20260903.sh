#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

trusted_sha="${1:?trusted k8s-cluster SHA required}"
[[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]]

organization='hhaus-org'
work="$(mktemp -d /tmp/hhaus-standard-repositories.XXXXXX)"
cleanup() {
  rc=$?
  trap - EXIT
  python3 - "$work" <<'PY'
import pathlib
import shutil
import sys
path = pathlib.Path(sys.argv[1])
if path.exists():
    shutil.rmtree(path)
PY
  exit "$rc"
}
trap cleanup EXIT

for command in bash chown find getent git install python3 stat sudo tail; do
  command -v "$command" >/dev/null || {
    printf 'hhaus-repository-provisioner=failed reason=missing-command command=%s\n' "$command" >&2
    exit 70
  }
done

# Fetch the exact reviewed k8s-cluster revision and install the repository's
# checksum-pinned GitHub CLI. The protected host intentionally uses its
# existing OAuth profile; personal access tokens and chat-provided credentials
# are rejected by this path.
git init "$work/k8s-cluster" >/dev/null
git -C "$work/k8s-cluster" remote add origin https://github.com/ORESoftware/k8s-cluster.git
git -C "$work/k8s-cluster" fetch --quiet --depth=1 origin "$trusted_sha"
git -C "$work/k8s-cluster" switch --quiet --detach FETCH_HEAD
actual_sha="$(git -C "$work/k8s-cluster" rev-parse HEAD)"
if test "$actual_sha" != "$trusted_sha"; then
  printf 'hhaus-repository-provisioner=failed reason=trusted-checkout-mismatch\n' >&2
  exit 71
fi

installer="$work/k8s-cluster/scripts/ops/install_pinned_github_cli.sh"
test -f "$installer" || {
  printf 'hhaus-repository-provisioner=failed reason=missing-pinned-gh-installer\n' >&2
  exit 71
}
gh_binary="$(bash "$installer" --install-dir "$work/pinned-gh")"
if test ! -x "$gh_binary"; then
  printf 'hhaus-repository-provisioner=failed reason=pinned-gh-not-executable\n' >&2
  exit 71
fi
case "$gh_binary" in
  "$work"/*/gh) ;;
  *)
    printf 'hhaus-repository-provisioner=failed reason=pinned-gh-path-escaped\n' >&2
    exit 71
    ;;
esac

profile_home="$(getent passwd ec2-user | awk -F: '$1 == "ec2-user" { print $6 }')"
profile_uid="$(id -u ec2-user)"
case "$profile_home" in
  /*) ;;
  *) printf 'hhaus-repository-profile=failed reason=missing-ec2-user\n' >&2; exit 65 ;;
esac

chown -R ec2-user:ec2-user "$work"
chmod 700 "$work"

selected_dir=''
diagnostic='no-candidate-profile'
while IFS= read -r candidate; do
  test -n "$candidate" || continue
  test -f "$candidate" || continue
  if test -L "$candidate"; then
    diagnostic='candidate-symlink-rejected'
    continue
  fi

  owner="$(stat -c '%u' "$candidate" 2>/dev/null || true)"
  mode="$(stat -c '%a' "$candidate" 2>/dev/null || true)"
  if test "$owner" != 0 && test "$owner" != "$profile_uid"; then
    diagnostic='candidate-owner-rejected'
    continue
  fi
  if [[ ! "$mode" =~ ^[0-7]{3,4}$ ]] || (( (8#$mode & 0022) != 0 )); then
    diagnostic='candidate-mode-rejected'
    continue
  fi

  candidate_dir="$work/gh-profile"
  python3 - "$candidate_dir" <<'PY'
import pathlib
import shutil
import sys
path = pathlib.Path(sys.argv[1])
if path.exists():
    shutil.rmtree(path)
PY
  install -d -m 700 -o ec2-user -g ec2-user "$candidate_dir"
  install -m 600 -o ec2-user -g ec2-user "$candidate" "$candidate_dir/hosts.yml"

  token="$(
    sudo -u ec2-user -H env \
      -u GH_TOKEN -u GITHUB_TOKEN -u GH_ENTERPRISE_TOKEN \
      -u GITHUB_REPOSITORY_ADMIN_TOKEN \
      GH_CONFIG_DIR="$candidate_dir" \
      "$gh_binary" auth token --hostname github.com 2>/dev/null || true
  )"
  if test -z "$token" || \
     [[ "$token" == *$'\n'* || "$token" == *$'\r'* || \
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
      unset token
      continue
      ;;
    gho_*|ghu_*|ghs_*) ;;
    *)
      diagnostic='candidate-token-class-rejected'
      unset token
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
  membership="$(
    sudo -u ec2-user -H env \
      -u GH_TOKEN -u GITHUB_TOKEN -u GH_ENTERPRISE_TOKEN \
      -u GITHUB_REPOSITORY_ADMIN_TOKEN \
      GH_CONFIG_DIR="$candidate_dir" \
      "$gh_binary" api "user/memberships/orgs/$organization" \
        --jq '[.role,.state] | @tsv' 2>/dev/null || true
  )"
  if test "$identity" != 'ORESoftware' || test "$membership" != $'admin\tactive'; then
    diagnostic='candidate-identity-or-org-role-rejected'
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

if test -z "$selected_dir"; then
  printf 'hhaus-repository-profile=failed reason=%s\n' "$diagnostic" >&2
  exit 65
fi

gh_as_admin() {
  sudo -u ec2-user -H env \
    -u GH_TOKEN -u GITHUB_TOKEN -u GH_ENTERPRISE_TOKEN \
    -u GITHUB_REPOSITORY_ADMIN_TOKEN \
    GH_CONFIG_DIR="$selected_dir" \
    "$gh_binary" "$@"
}

repositories=(
  'hhaus-clients|Transport-injected H/HAUS clients in 15+ languages, resolved through zed-pkg'
  'hhaus-sync|Offline-first H/HAUS synchronization across app, IndexedDB, SQLite, Supabase and Postgres layers'
  'hhaus-lib-core|Shared H/HAUS client, server, edge and isomorphic domain and policy primitives'
  'hhaus-orm-core|Backend-only H/HAUS Diesel and SeaORM persistence implementations'
  'hhaus-flutter|Cross-platform H/HAUS Flutter application for iOS, Android, web and desktop'
  'hhaus-desktop-app.rs|Native Rust H/HAUS desktop application'
  'hhaus-lambdas|Provider-neutral cross-platform H/HAUS function runtime and cloud adapters'
  'hhaus-interfaces|Peer-authority TypeSpec and JSON Schema contracts with 15+ generated language surfaces'
)

for entry in "${repositories[@]}"; do
  name="${entry%%|*}"
  description="${entry#*|}"
  repository="$organization/$name"
  created=false

  if metadata="$(gh_as_admin api "repos/$repository" 2>/dev/null)"; then
    :
  else
    gh_as_admin repo create "$repository" \
      --private \
      --description "$description" \
      --add-readme \
      --disable-wiki >/dev/null
    created=true
  fi

  gh_as_admin api --method PATCH "repos/$repository" \
    -f description="$description" \
    -F has_issues=true \
    -F has_projects=false \
    -F has_wiki=false \
    -F allow_squash_merge=true \
    -F allow_merge_commit=true \
    -F allow_rebase_merge=false \
    -F delete_branch_on_merge=true >/dev/null

  metadata="$(gh_as_admin api "repos/$repository")"
  python3 - "$metadata" "$repository" <<'PY'
import json
import sys
value = json.loads(sys.argv[1])
expected = sys.argv[2]
if value.get("full_name") != expected:
    raise SystemExit(f"repository identity mismatch: {value.get('full_name')!r}")
if value.get("private") is not True or value.get("visibility") != "private":
    raise SystemExit("repository must be private")
if value.get("default_branch") != "main":
    raise SystemExit(f"default branch must be main: {value.get('default_branch')!r}")
if value.get("archived") is True or value.get("disabled") is True:
    raise SystemExit("repository is archived or disabled")
if value.get("has_issues") is not True or value.get("has_wiki") is not False:
    raise SystemExit("repository feature settings differ")
if value.get("allow_rebase_merge") is not False:
    raise SystemExit("rebase merging must be disabled")
PY

  main_sha="$(gh_as_admin api "repos/$repository/git/ref/heads/main" --jq .object.sha)"
  [[ "$main_sha" =~ ^[0-9a-f]{40}$ ]]
  printf 'HHAUS_REPOSITORY_READY repository=%s created=%s main=%s\n' \
    "$repository" "$created" "$main_sha"
done

printf 'HHAUS_REPOSITORY_FLEET_READY count=%s trusted_k8s_cluster_sha=%s\n' \
  "${#repositories[@]}" "$trusted_sha"

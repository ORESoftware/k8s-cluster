#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

trusted_sha="${1:?trusted k8s-cluster SHA required}"
[[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]]

repository='benefactor-cc/benefactor-gas'
description='Consent-gated Google Apps Script and Cloudflare gateway for Benefactor outreach'
work="$(mktemp -d /tmp/benefactor-gas-repository.XXXXXX)"
cleanup() {
  rc=$?
  trap - EXIT
  rm -rf "$work"
  exit "$rc"
}
trap cleanup EXIT

command -v gh >/dev/null
command -v sudo >/dev/null
command -v getent >/dev/null
command -v install >/dev/null
command -v stat >/dev/null
command -v python3 >/dev/null

profile_home="$(getent passwd ec2-user | awk -F: '$1 == "ec2-user" { print $6 }')"
profile_uid="$(id -u ec2-user)"
case "$profile_home" in
  /*) ;;
  *) echo 'benefactor-gas-profile=failed reason=missing-ec2-user' >&2; exit 65 ;;
esac

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
  rm -rf "$candidate_dir"
  install -d -m 700 -o ec2-user -g ec2-user "$candidate_dir"
  install -m 600 -o ec2-user -g ec2-user "$candidate" "$candidate_dir/hosts.yml"

  token="$(
    sudo -u ec2-user -H env \
      -u GH_TOKEN -u GITHUB_TOKEN -u GH_ENTERPRISE_TOKEN \
      -u GITHUB_REPOSITORY_ADMIN_TOKEN \
      GH_CONFIG_DIR="$candidate_dir" \
      gh auth token --hostname github.com 2>/dev/null || true
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
      gh api user --jq .login 2>/dev/null || true
  )"
  membership="$(
    sudo -u ec2-user -H env \
      -u GH_TOKEN -u GITHUB_TOKEN -u GH_ENTERPRISE_TOKEN \
      -u GITHUB_REPOSITORY_ADMIN_TOKEN \
      GH_CONFIG_DIR="$candidate_dir" \
      gh api user/memberships/orgs/benefactor-cc \
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
  printf 'benefactor-gas-profile=failed reason=%s\n' "$diagnostic" >&2
  exit 65
fi

gh_as_admin() {
  sudo -u ec2-user -H env \
    -u GH_TOKEN -u GITHUB_TOKEN -u GH_ENTERPRISE_TOKEN \
    -u GITHUB_REPOSITORY_ADMIN_TOKEN \
    GH_CONFIG_DIR="$selected_dir" \
    gh "$@"
}

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
  metadata="$(gh_as_admin api "repos/$repository")"
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
PY

main_sha="$(gh_as_admin api "repos/$repository/git/ref/heads/main" --jq .object.sha)"
[[ "$main_sha" =~ ^[0-9a-f]{40}$ ]]
printf 'BENEFACTOR_GAS_REPOSITORY_READY repository=%s created=%s main=%s trusted_k8s_cluster_sha=%s\n' \
  "$repository" "$created" "$main_sha" "$trusted_sha"

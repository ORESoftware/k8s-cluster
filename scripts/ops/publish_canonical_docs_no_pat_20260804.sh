#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

repo_root="${1:-$(git rev-parse --show-toplevel)}"
repo_root="$(cd "$repo_root" && pwd)"
carrier="$repo_root/ops/canonical-docs"
dry_run="${PUBLISH_DRY_RUN:-0}"
main_sha='86fb7c44ac88f2f4e5f9ff314c50cac736f63789'
feature_sha='07da928d1b80aeca10c8d29daa26a967be1748dd'
feature='agent/den-1049-publish-business-plan'
archive_sha='0651f887fc6189317209b6c41903872d1b49c3f179ebfcde8ef968e54ebe6af1'
archive_size='35233'
business_plan_sha='db82c704fd3cfc472ed0cc77e2fc15acad8641929441c3772137920a735960f8'

stage=initialization
work="$(mktemp -d /tmp/canonical-docs-publisher.XXXXXX)"
cleanup() {
  unset token GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN
  rm -rf "$work"
}
report_failure() {
  local rc=$?
  trap - ERR
  printf 'canonical-docs-stage=%s status=failed rc=%s\n' "$stage" "$rc" >&2
  exit "$rc"
}
trap cleanup EXIT
trap report_failure ERR

stage=carrier-inventory
python3 - "$carrier" <<'PY'
import os, stat, sys
from pathlib import Path
root = Path(sys.argv[1]).resolve(strict=True)
if root.is_symlink() or not root.is_dir():
    raise SystemExit("carrier must be a real directory")
expected = {
    "README.md", "manifest.json", "pr-body.md",
    *{f"source.tar.gz.b64.part{i:03d}" for i in range(4)},
}
actual = {path.name for path in root.iterdir()}
if actual != expected:
    raise SystemExit(f"carrier inventory differs: missing={expected-actual}, unexpected={actual-expected}")
for path in root.iterdir():
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode):
        raise SystemExit(f"carrier entry must be regular: {path.name}")
    path.resolve(strict=True).relative_to(root)
PY
printf 'canonical-docs-stage=%s status=passed\n' "$stage"

stage=source-reconstruction
archive="$work/canonical-docs-source.tar.gz"
cat "$carrier"/source.tar.gz.b64.part??? | base64 --decode > "$archive"
test "$(wc -c < "$archive")" = "$archive_size"
test "$(sha256sum "$archive" | awk '{print $1}')" = "$archive_sha"
source_tree="$work/source-tree"
mkdir -p "$source_tree"
python3 - "$archive" "$source_tree" <<'PY'
import hashlib, stat, sys, tarfile
from pathlib import Path, PurePosixPath
archive, destination = map(Path, sys.argv[1:])
with tarfile.open(archive, "r:gz") as handle:
    files = 0
    for member in handle.getmembers():
        path = PurePosixPath(member.name)
        if path.is_absolute() or any(part in {"", ".", ".."} for part in path.parts):
            raise SystemExit(f"unsafe archive path: {member.name!r}")
        if member.issym() or member.islnk() or member.isdev() or member.isfifo():
            raise SystemExit(f"unsafe archive entry: {member.name!r}")
        if not (member.isdir() or member.isfile()):
            raise SystemExit(f"unsupported archive entry: {member.name!r}")
        if member.isfile():
            files += 1
            if member.mode & 0o777 not in {0o644, 0o664, 0o755, 0o775}:
                raise SystemExit(f"unsafe archive mode: {member.name!r}")
    if files != 26:
        raise SystemExit(f"expected 26 source files, found {files}")
    handle.extractall(destination, filter="data")
PY
test "$(sha256sum "$source_tree/docs/business-plan.md" | awk '{print $1}')" = "$business_plan_sha"
python3 "$source_tree/scripts/check_docs.py"
python3 -m unittest discover -s "$source_tree/tests" -v
printf 'canonical-docs-stage=%s status=passed\n' "$stage"

stage=deterministic-history
source="$work/source"
git init --quiet -b main "$source"
git -C "$source" config user.name 'Canonical Cloud Automation'
git -C "$source" config user.email 'automation@canonical.cloud'
git -C "$source" config commit.gpgsign false
git -C "$source" config core.autocrlf false
git -C "$source" config core.filemode true
cp "$source_tree/.gitignore" "$source/.gitignore"
cp "$source_tree/README.md" "$source/README.md"
git -C "$source" add -- .gitignore README.md
GIT_AUTHOR_NAME='Canonical Cloud Automation' \
GIT_AUTHOR_EMAIL='automation@canonical.cloud' \
GIT_COMMITTER_NAME='Canonical Cloud Automation' \
GIT_COMMITTER_EMAIL='automation@canonical.cloud' \
GIT_AUTHOR_DATE='2026-08-04T20:00:00Z' \
GIT_COMMITTER_DATE='2026-08-04T20:00:00Z' \
  git -C "$source" commit --quiet -m 'Bootstrap canonical-docs repository'
git -C "$source" switch --quiet -c "$feature"
cp -a "$source_tree/." "$source/"
git -C "$source" add -A
GIT_AUTHOR_NAME='Canonical Cloud Automation' \
GIT_AUTHOR_EMAIL='automation@canonical.cloud' \
GIT_COMMITTER_NAME='Canonical Cloud Automation' \
GIT_COMMITTER_EMAIL='automation@canonical.cloud' \
GIT_AUTHOR_DATE='2026-08-04T20:01:00Z' \
GIT_COMMITTER_DATE='2026-08-04T20:01:00Z' \
  git -C "$source" commit --quiet -m 'Publish Canonical Cloud business plan and documentation baseline'
test "$(git -C "$source" rev-parse main)" = "$main_sha"
test "$(git -C "$source" rev-parse "$feature")" = "$feature_sha"
test "$(git -C "$source" rev-parse 'main^{tree}')" = '5cd9c62d21b4a32d34b886b037849d27258e287c'
test "$(git -C "$source" rev-parse "$feature^{tree}")" = '8a3ee9d5f610bb4f7b6ba63220a7c7e9291a12ad'
git -C "$source" diff --check "main...$feature"
printf 'canonical-docs-stage=%s status=passed main=%s feature=%s\n' "$stage" "$main_sha" "$feature_sha"

if test "$dry_run" = 1; then
  printf '{"status":"validated","repository":"canonical-cloud/canonical-docs","main":"%s","feature":"%s"}\n' \
    "$main_sha" "$feature_sha"
  exit 0
fi

stage=protected-gh-profile
command -v sudo >/dev/null 2>&1
command -v getent >/dev/null 2>&1
profile_home="$(getent passwd ec2-user | awk -F: '$1 == "ec2-user" { print $6 }')"
case "$profile_home" in /*) ;; *) printf 'missing protected ec2-user GitHub profile\n' >&2; exit 65 ;; esac

token="$(
  sudo -u ec2-user -H env \
    -u GH_TOKEN -u GITHUB_TOKEN -u GH_ENTERPRISE_TOKEN \
    -u GITHUB_REPOSITORY_ADMIN_TOKEN -u GH_CONFIG_DIR \
    HOME="$profile_home" XDG_CONFIG_HOME="$profile_home/.config" \
    bash -c 'command -v gh >/dev/null 2>&1 && gh auth token --hostname github.com' \
    2>/dev/null
)"
test -n "$token"
[[ "$token" != *$'\n'* && "$token" != *$'\r'* && "$token" != *$'\t'* && "$token" != *' '* ]]
case "$token" in
  ghp_*|github_pat_*)
    printf 'protected GitHub profile uses a personal access token; refusing publication\n' >&2
    exit 65
    ;;
esac
unset token
sudo -u ec2-user -H env \
  -u GH_TOKEN -u GITHUB_TOKEN -u GH_ENTERPRISE_TOKEN \
  -u GITHUB_REPOSITORY_ADMIN_TOKEN -u GH_CONFIG_DIR \
  HOME="$profile_home" XDG_CONFIG_HOME="$profile_home/.config" \
  gh auth status --hostname github.com >/dev/null
printf 'canonical-docs-stage=%s status=passed source=protected-gh-oauth-profile\n' "$stage"

stage=repository-pr-ci-merge
cp "$carrier/pr-body.md" "$work/pr-body.md"
chown -R ec2-user:ec2-user "$source" "$work/pr-body.md"
profile_script="$work/publish-profile.sh"
cat > "$profile_script" <<'PROFILE_SH'
#!/usr/bin/env bash
set -Eeuo pipefail
umask 077
source="${1:?source repository required}"
pr_body="${2:?PR body required}"
repository='canonical-cloud/canonical-docs'
feature='agent/den-1049-publish-business-plan'
main_sha='86fb7c44ac88f2f4e5f9ff314c50cac736f63789'
feature_sha='07da928d1b80aeca10c8d29daa26a967be1748dd'
plan_sha='db82c704fd3cfc472ed0cc77e2fc15acad8641929441c3772137920a735960f8'

gh auth status --hostname github.com >/dev/null
gh auth setup-git --hostname github.com >/dev/null
if repository_json="$(gh repo view "$repository" --json nameWithOwner,visibility 2>/dev/null)"; then
  python3 - "$repository_json" <<'PY'
import json, sys
value = json.loads(sys.argv[1])
if value != {"nameWithOwner": "canonical-cloud/canonical-docs", "visibility": "PUBLIC"}:
    raise SystemExit(f"existing repository settings differ: {value!r}")
PY
else
  gh repo create "$repository" --public \
    --description 'Canonical Cloud strategy, business, compliance, and operating documentation' \
    --disable-wiki
fi

git -C "$source" remote add origin "https://github.com/$repository.git" 2>/dev/null || \
  git -C "$source" remote set-url origin "https://github.com/$repository.git"
# Never force: any unexpected history or concurrent owner work fails closed.
git -C "$source" push origin "main:refs/heads/main"
git -C "$source" push origin "$feature:refs/heads/$feature"
test "$(git ls-remote "https://github.com/$repository.git" refs/heads/main | awk '{print $1}')" = "$main_sha"
test "$(git ls-remote "https://github.com/$repository.git" "refs/heads/$feature" | awk '{print $1}')" = "$feature_sha"

gh api --method PATCH "repos/$repository" \
  -f default_branch=main -F has_issues=true -F has_projects=false -F has_wiki=false \
  -F allow_squash_merge=true -F allow_merge_commit=true -F allow_rebase_merge=false \
  -F delete_branch_on_merge=true >/dev/null

settings="$(gh repo view "$repository" --json nameWithOwner,visibility,defaultBranchRef)"
python3 - "$settings" <<'PY'
import json, sys
value = json.loads(sys.argv[1])
if value.get("nameWithOwner") != "canonical-cloud/canonical-docs":
    raise SystemExit(f"repository identity differs: {value!r}")
if value.get("visibility") != "PUBLIC":
    raise SystemExit(f"repository is not public: {value!r}")
if (value.get("defaultBranchRef") or {}).get("name") != "main":
    raise SystemExit(f"default branch is not main: {value!r}")
PY

pulls="$(gh api "repos/$repository/pulls?state=all&head=canonical-cloud:$feature&base=main&per_page=100")"
pr_number="$(python3 - "$pulls" "$feature_sha" <<'PY'
import json, sys
matching = [pr for pr in json.loads(sys.argv[1]) if pr["head"]["sha"] == sys.argv[2]]
if len(matching) > 1:
    raise SystemExit("multiple PRs match the exact head")
if matching:
    print(matching[0]["number"])
PY
)"
if test -z "$pr_number"; then
  pr_url="$(gh pr create --repo "$repository" --base main --head "$feature" \
    --title 'Publish Canonical Cloud business plan' --body-file "$pr_body")"
  pr_number="${pr_url##*/}"
else
  pr_url="$(gh pr view "$pr_number" --repo "$repository" --json url --jq .url)"
fi
test "$(gh pr view "$pr_number" --repo "$repository" --json headRefOid --jq .headRefOid)" = "$feature_sha"
echo "source-pr=$pr_url"

run_id=''; run_status=''; run_conclusion=''; run_url=''
for _ in $(seq 1 120); do
  runs="$(gh api "repos/$repository/actions/runs?branch=$feature&event=pull_request&per_page=50")"
  selection="$(python3 - "$runs" "$feature_sha" <<'PY'
import json, sys
for run in json.loads(sys.argv[1]).get("workflow_runs", []):
    if run.get("head_sha") == sys.argv[2] and run.get("name") == "Documentation contract":
        print(f"{run['id']}\t{run['status']}\t{run.get('conclusion') or ''}\t{run['html_url']}")
        break
PY
)"
  if test -n "$selection"; then
    IFS=$'\t' read -r run_id run_status run_conclusion run_url <<< "$selection"
    if test "$run_status" = completed; then test "$run_conclusion" = success; break; fi
  fi
  sleep 5
done
test -n "$run_id"; test "$run_status" = completed; test "$run_conclusion" = success
echo "source-ci=$run_url"

merge="$(gh api --method PUT "repos/$repository/pulls/$pr_number/merge" \
  -f merge_method=squash -f sha="$feature_sha" \
  -f commit_title="Publish Canonical Cloud business plan (#$pr_number)" \
  -f commit_message=$'Publish the evidence-bounded business plan, claims register, repository boundaries, security and contribution policy, canonical agent instructions, and pinned documentation CI.\n\nCertified on exact feature head 07da928d1b80aeca10c8d29daa26a967be1748dd.\n\nRefs DEN-1049\nRelated: DEN-319, DEN-621, DEN-628, DEN-127')"
merge_sha="$(python3 - "$merge" <<'PY'
import json, sys
value = json.loads(sys.argv[1])
if value.get("merged") is not True:
    raise SystemExit(f"PR was not merged: {value!r}")
print(value["sha"])
PY
)"
test "$(gh api "repos/$repository/commits/main" --jq .sha)" = "$merge_sha"
verify="$(mktemp -d /tmp/canonical-docs-final.XXXXXX)"
trap 'rm -rf "$verify"' EXIT
git clone --quiet --branch main "https://github.com/$repository.git" "$verify"
python3 "$verify/scripts/check_docs.py"
python3 -m unittest discover -s "$verify/tests" -v
test "$(sha256sum "$verify/docs/business-plan.md" | awk '{print $1}')" = "$plan_sha"
echo "repository-url=https://github.com/$repository"
echo "merged-pr=$pr_url"
echo "merge-sha=$merge_sha"
echo "business-plan-url=https://github.com/$repository/blob/main/docs/business-plan.md"
PROFILE_SH
chmod 700 "$profile_script"
chown ec2-user:ec2-user "$profile_script"
sudo -u ec2-user -H env \
  -u GH_TOKEN -u GITHUB_TOKEN -u GH_ENTERPRISE_TOKEN \
  -u GITHUB_REPOSITORY_ADMIN_TOKEN -u GH_CONFIG_DIR \
  HOME="$profile_home" XDG_CONFIG_HOME="$profile_home/.config" \
  bash "$profile_script" "$source" "$work/pr-body.md"
printf 'canonical-docs-stage=%s status=passed\n' "$stage"
printf 'canonical-docs-stage=complete status=success\n'

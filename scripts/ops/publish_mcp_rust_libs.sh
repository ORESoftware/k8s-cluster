#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

stage=bootstrap
report_error() {
  status=$?
  trap - ERR
  printf 'publisher-stage-failed=%s exit=%s\n' "$stage" "$status" >&2
  exit "$status"
}
trap report_error ERR

trusted_sha="${1:?trusted k8s-cluster SHA required}"
[[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]]

readonly expected_login='ORESoftware'
readonly target_repository='ORESoftware/mcp-rust-libs'
readonly source_repository='ORESoftware/testing'
readonly source_sha='069b1aa4251658c8348d2eb477ad71369d9b742b'
readonly source_subdirectory='mcp-rust-libs'
readonly source_manifest_sha256='b9ba89f29dca3e5020430d3a5d35967e523d3e94db9168a91cdf24a9bd5f2a33'
readonly target_branch='bootstrap/semantic-polyglot-20260731'
readonly baseline_timestamp='2026-07-31T13:30:00Z'
readonly source_timestamp='2026-07-31T13:31:00Z'

test -z "${CODEX_HOME:-}"
stage=prepare-disposable-state
work="$(mktemp -d /tmp/mcp-rust-libs-publisher.XXXXXX)"
export CODEX_HOME="$work/codex-home"
install -d -m 700 "$CODEX_HOME"
case "$CODEX_HOME" in
  "$work"/*) ;;
  *) echo 'disposable CODEX_HOME escaped the temporary worktree' >&2; exit 1 ;;
esac
cleanup() {
  unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN \
    GIT_ASKPASS GIT_ASKPASS_REQUIRE GIT_USERNAME GIT_TERMINAL_PROMPT \
    GIT_CONFIG_GLOBAL GIT_CONFIG_NOSYSTEM CODEX_HOME encoded_pat
  rm -rf "$work"
}
trap cleanup EXIT

stage=receive-protected-credential
IFS= read -r encoded_pat
test -n "$encoded_pat"
GH_TOKEN="$(printf '%s' "$encoded_pat" | base64 --decode)"
unset encoded_pat
test -n "$GH_TOKEN"
[[ "$GH_TOKEN" != *[[:space:]]* ]]
export GH_TOKEN
export GITHUB_REPOSITORY_ADMIN_TOKEN="$GH_TOKEN"
printf 'publisher-stage=%s status=ready source=protected-broker-stdin\n' "$stage"

stage=verify-github-identity
api_helper="$work/github_api.py"
cat > "$api_helper" <<'PYTHON'
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request

API = "https://api.github.com"
TOKEN = os.environ["GH_TOKEN"]
TARGET = "ORESoftware/mcp-rust-libs"
OWNER = "ORESoftware"
DESCRIPTION = (
    "Shared Rust, TypeScript, Dart, and Gleam MCP runtime, contracts, "
    "safety, telemetry, code generation, and testkit libraries"
)
HEADERS = {
    "Accept": "application/vnd.github+json",
    "Authorization": f"Bearer {TOKEN}",
    "X-GitHub-Api-Version": "2022-11-28",
    "User-Agent": "bounded-mcp-rust-libs-publisher",
}

def request(method: str, path: str, payload=None, allow_404: bool = False):
    body = None
    headers = dict(HEADERS)
    if payload is not None:
        body = json.dumps(payload, separators=(",", ":")).encode()
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(API + path, data=body, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=30) as response:
            raw = response.read()
    except urllib.error.HTTPError as error:
        if allow_404 and error.code == 404:
            return None
        error.read(4096)
        raise SystemExit(f"GitHub API {method} {path} failed with HTTP {error.code}")
    return json.loads(raw) if raw else None

def ensure_repository():
    repo = request("GET", f"/repos/{TARGET}", allow_404=True)
    if repo is None:
        repo = request(
            "POST",
            "/user/repos",
            {
                "name": "mcp-rust-libs",
                "description": DESCRIPTION,
                "private": False,
                "has_issues": True,
                "has_projects": False,
                "has_wiki": False,
                "auto_init": False,
            },
        )
        print(f"CREATED {TARGET}")
    if repo.get("owner", {}).get("login") != OWNER or repo.get("visibility") != "public":
        raise SystemExit("canonical repository owner or visibility is not exact")
    request(
        "PATCH",
        f"/repos/{TARGET}",
        {
            "description": DESCRIPTION,
            "private": False,
            "has_issues": True,
            "has_projects": False,
            "has_wiki": False,
            "allow_squash_merge": True,
            "allow_merge_commit": True,
            "allow_rebase_merge": False,
            "delete_branch_on_merge": True,
        },
    )

def get_ref(ref: str) -> str:
    value = request("GET", f"/repos/{TARGET}/git/ref/{ref}", allow_404=True)
    return "" if value is None else value["object"]["sha"]

def ensure_pull_request(branch: str, expected_head: str, body: str) -> int:
    query = urllib.parse.urlencode(
        {
            "state": "all",
            "head": f"{OWNER}:{branch}",
            "base": "main",
            "per_page": 10,
        }
    )
    pulls = request("GET", f"/repos/{TARGET}/pulls?{query}")
    if len(pulls) > 1:
        raise SystemExit("multiple canonical bootstrap pull requests exist")
    if not pulls:
        pull = request(
            "POST",
            f"/repos/{TARGET}/pulls",
            {
                "title": "Bootstrap shared polyglot MCP libraries",
                "head": branch,
                "base": "main",
                "body": body,
                "maintainer_can_modify": True,
            },
        )
    else:
        pull = pulls[0]
        if pull.get("merged_at"):
            raise SystemExit("bootstrap pull request is merged but main is not the reviewed tree")
        if pull.get("state") == "closed":
            pull = request(
                "PATCH",
                f"/repos/{TARGET}/pulls/{pull['number']}",
                {"state": "open"},
            )
    if pull.get("head", {}).get("sha") != expected_head:
        raise SystemExit("bootstrap pull request head differs from reviewed branch head")
    return int(pull["number"])

command = sys.argv[1]
if command == "user":
    print(request("GET", "/user")["login"])
elif command == "ensure-repository":
    ensure_repository()
elif command == "get-ref":
    print(get_ref(sys.argv[2]))
elif command == "set-default-main":
    request("PATCH", f"/repos/{TARGET}", {"default_branch": "main"})
elif command == "ensure-pr":
    branch, expected_head, body_path = sys.argv[2:5]
    body = open(body_path, encoding="utf-8").read()
    print(ensure_pull_request(branch, expected_head, body))
else:
    raise SystemExit(f"unknown helper command: {command}")
PYTHON
chmod 700 "$api_helper"

actual_login="$(python3 "$api_helper" user)"
test "$actual_login" = "$expected_login"

stage=configure-git-authentication
askpass="$work/git-askpass.sh"
cat > "$askpass" <<'ASKPASS'
#!/usr/bin/env bash
case "${1:-}" in
  *Username*) printf '%s\n' "${GIT_USERNAME:-x-access-token}" ;;
  *) printf '%s\n' "${GH_TOKEN:?missing GH_TOKEN}" ;;
esac
ASKPASS
chmod 700 "$askpass"
export GIT_ASKPASS="$askpass"
export GIT_ASKPASS_REQUIRE=force
export GIT_USERNAME='x-access-token'
export GIT_TERMINAL_PROMPT=0
export GIT_CONFIG_GLOBAL=/dev/null
export GIT_CONFIG_NOSYSTEM=1

stage=checkout-reviewed-source
carrier="$work/carrier"
git clone --filter=blob:none --no-checkout \
  "https://github.com/${source_repository}.git" "$carrier"
git -C "$carrier" fetch --depth=1 origin "$source_sha"
git -C "$carrier" checkout --detach "$source_sha"
test "$(git -C "$carrier" rev-parse HEAD)" = "$source_sha"

source="$carrier/$source_subdirectory"
for required in \
  Cargo.toml \
  .zpkg.toml \
  .github/workflows/scaffold.yml \
  packages/rust/Cargo.toml \
  packages/typescript/package.json \
  packages/dart/pubspec.yaml \
  packages/gleam/gleam.toml \
  reports/polyglot-api-repair-complete.txt \
  reports/source-files.sha256; do
  test -f "$source/$required"
done
test "$(sha256sum "$source/reports/source-files.sha256" | awk '{print $1}')" = \
  "$source_manifest_sha256"
source_tree="$(git -C "$carrier" rev-parse "${source_sha}:${source_subdirectory}")"
[[ "$source_tree" =~ ^[0-9a-f]{40}$ ]]

stage=validate-reviewed-source
(
  cd "$source"
  python3 scripts/regenerate-generated.py --check
  python3 scripts/static-source-checks.py
  python3 scripts/check-scaffold.py
  python3 tooling/conformance/run.py
  python3 scripts/update-source-manifest.py --check
)

stage=ensure-target-repository
python3 "$api_helper" ensure-repository

stage=prepare-target-review-gate
baseline="$work/baseline"
mkdir -p "$baseline/.github/workflows"
git -C "$baseline" init -b main
git -C "$baseline" config user.name 'ORESoftware publication automation'
git -C "$baseline" config user.email 'bot@oresoftware.dev'
cp "$source/README.md" "$baseline/README.md"
cp "$source/LICENSE-MIT" "$baseline/LICENSE-MIT"
cp "$source/LICENSE-APACHE" "$baseline/LICENSE-APACHE"
cp "$source/CODEOWNERS" "$baseline/CODEOWNERS"
cp "$source/.github/workflows/scaffold.yml" \
  "$baseline/.github/workflows/scaffold.yml"
git -C "$baseline" add -A
GIT_AUTHOR_DATE="$baseline_timestamp" GIT_COMMITTER_DATE="$baseline_timestamp" \
  git -C "$baseline" commit -m 'chore: initialize canonical repository and review gate [skip ci]'
expected_main="$(git -C "$baseline" rev-parse HEAD)"
expected_main_tree="$(git -C "$baseline" rev-parse 'HEAD^{tree}')"

remote_main="$(python3 "$api_helper" get-ref heads/main)"
if test -z "$remote_main"; then
  git -C "$baseline" remote add origin "https://github.com/${target_repository}.git"
  git -C "$baseline" -c credential.helper= push origin HEAD:refs/heads/main
  remote_main="$(python3 "$api_helper" get-ref heads/main)"
  test "$remote_main" = "$expected_main"
  python3 "$api_helper" set-default-main
fi

stage=publish-reviewed-source-branch
target="$work/target"
git clone --depth=1 --branch main "https://github.com/${target_repository}.git" "$target"
main_tree="$(git -C "$target" rev-parse 'HEAD^{tree}')"
if test "$main_tree" = "$source_tree"; then
  echo "ALREADY_PUBLISHED $target_repository main=$remote_main source_tree=$source_tree trusted_k8s=$trusted_sha"
  exit 0
fi
if test "$main_tree" != "$expected_main_tree"; then
  echo "Refusing unexpected $target_repository/main: commit=$remote_main tree=$main_tree" >&2
  exit 1
fi

remote_head="$(git -C "$target" ls-remote origin "refs/heads/${target_branch}" | awk '{print $1}')"
if test -n "$remote_head"; then
  git -C "$target" fetch --depth=1 origin "$target_branch"
  branch_tree="$(git -C "$target" rev-parse 'FETCH_HEAD^{tree}')"
  if test "$branch_tree" != "$source_tree"; then
    echo "Refusing divergent $target_repository/$target_branch: head=$remote_head tree=$branch_tree" >&2
    exit 1
  fi
  expected_head="$remote_head"
else
  git -C "$target" checkout -b "$target_branch" main
  find "$target" -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf {} +
  cp -a "$source"/. "$target"/
  git -C "$target" config user.name 'ORESoftware publication automation'
  git -C "$target" config user.email 'bot@oresoftware.dev'
  git -C "$target" add -A
  GIT_AUTHOR_DATE="$source_timestamp" GIT_COMMITTER_DATE="$source_timestamp" \
    git -C "$target" commit \
      -m 'feat: bootstrap shared polyglot MCP libraries' \
      -m "Promote reviewed ${source_repository}@${source_sha} subtree to canonical repository root."
  expected_head="$(git -C "$target" rev-parse HEAD)"
  test "$(git -C "$target" rev-parse 'HEAD^{tree}')" = "$source_tree"
  git -C "$target" -c credential.helper= push origin "HEAD:refs/heads/${target_branch}"
  test "$(python3 "$api_helper" get-ref "heads/${target_branch}")" = "$expected_head"
fi

pr_body="$work/bootstrap-pr.md"
cat > "$pr_body" <<EOF
## Canonical bootstrap

This promotes only the exact reviewed \`${source_subdirectory}/\` subtree from
\`${source_repository}@${source_sha}\` to repository root.

The deep semantic merge preserves the hardened Rust runtime, strict
configuration, stderr-only telemetry, bounded HTTP, safety, and
real-process testkit implementation while layering shared JSON Schema
contracts, deterministic code generation, TypeScript/Zod, Dart, Gleam,
portable fixtures, and the four-target Zed package around it.

Product tools, credentials, authorization, mutation policy, endpoint
ownership, and business schemas remain in their owning repositories.

Source-manifest SHA-256: \`${source_manifest_sha256}\`.
Source Git tree: \`${source_tree}\`.
Merge only after the complete target polyglot matrix succeeds.

Refs DEN-319, DEN-957, DEN-959, DEN-967, DEN-968, DEN-969, DEN-970, DEN-972, DEN-1186.
EOF
stage=ensure-target-pull-request
pr_number="$(python3 "$api_helper" ensure-pr "$target_branch" "$expected_head" "$pr_body")"
test "$(python3 "$api_helper" get-ref "heads/${target_branch}")" = "$expected_head"
stage=complete
echo "PUBLISHED $target_repository PR#$pr_number head=$expected_head source_tree=$source_tree trusted_k8s=$trusted_sha"

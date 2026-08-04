#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

stage=initialization
work="$(mktemp -d /tmp/agent-pontifex-fiducia-publisher.XXXXXX)"

on_error() {
  local rc=$?
  trap - ERR
  printf 'publisher status=failed stage=%s rc=%s\n' "$stage" "$rc" >&2
  exit "$rc"
}

cleanup() {
  unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN
  unset GIT_ASKPASS GIT_ASKPASS_REQUIRE GIT_TERMINAL_PROMPT
  unset GIT_CONFIG_COUNT GIT_CONFIG_KEY_0 GIT_CONFIG_VALUE_0
  rm -rf "$work"
}

trap on_error ERR
trap cleanup EXIT

for tool in git python3; do
  command -v "$tool" >/dev/null
  printf 'preflight tool=%s status=present\n' "$tool"
done

for inherited in GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN GIT_ASKPASS; do
  if test -n "${!inherited:-}"; then
    printf 'publisher status=failed stage=%s reason=inherited-%s\n' "$stage" "$inherited" >&2
    exit 64
  fi
done

stage=protected-github-profile
gh_hosts="${GH_CONFIG_DIR:-$HOME/.config/gh}/hosts.yml"
test -r "$gh_hosts"
GH_TOKEN="$(
  GH_HOSTS="$gh_hosts" python3 - <<'PY'
import ast
import os
import re
import sys
from pathlib import Path

path = Path(os.environ["GH_HOSTS"])
current_host = None
for raw in path.read_text(encoding="utf-8").splitlines():
    stripped = raw.strip()
    if not stripped or stripped.startswith("#"):
        continue
    indent = len(raw) - len(raw.lstrip())
    if indent == 0 and stripped.endswith(":"):
        current_host = stripped[:-1]
        continue
    if current_host != "github.com":
        continue
    match = re.match(r"^\s+oauth_token:\s*(.*?)\s*$", raw)
    if match is None:
        continue
    value = match.group(1)
    if len(value) >= 2 and value[0] == value[-1] and value[0] in "'\"":
        try:
            value = ast.literal_eval(value)
        except (SyntaxError, ValueError) as error:
            raise SystemExit(65) from error
    if not isinstance(value, str) or not value or any(ch.isspace() for ch in value):
        raise SystemExit(65)
    sys.stdout.write(value)
    raise SystemExit(0)
raise SystemExit(65)
PY
)"
test -n "$GH_TOKEN"
case "$GH_TOKEN" in
  *$'\n'*|*$'\r'*|*$'\t'*|*' '*) exit 65 ;;
esac
export GH_TOKEN
export GITHUB_REPOSITORY_ADMIN_TOKEN="$GH_TOKEN"
printf 'publisher stage=%s status=ready\n' "$stage"

api_helper="$work/github-api.py"
cat >"$api_helper" <<'PY'
#!/usr/bin/env python3
import os
import sys
import urllib.error
import urllib.request

method, path = sys.argv[1:3]
body = sys.stdin.buffer.read()
headers = {
    "Accept": "application/vnd.github+json",
    "Authorization": f"Bearer {os.environ['GH_TOKEN']}",
    "User-Agent": "agent-pontifex-fiducia-publisher",
    "X-GitHub-Api-Version": "2022-11-28",
}
if body:
    headers["Content-Type"] = "application/json"
request = urllib.request.Request(
    "https://api.github.com" + path,
    data=body or None,
    headers=headers,
    method=method,
)
try:
    with urllib.request.urlopen(request, timeout=60) as response:
        payload = response.read()
except urllib.error.HTTPError as error:
    error.read(8192)
    print(f"GitHub API {method} {path} failed with HTTP {error.code}", file=sys.stderr)
    raise SystemExit(44 if error.code == 404 else 1) from error
if payload:
    sys.stdout.buffer.write(payload)
PY
chmod 700 "$api_helper"

api_call() {
  python3 "$api_helper" "$1" "$2"
}

json_get() {
  local path="$1"
  python3 -c '
import json
import sys
value = json.load(sys.stdin)
for part in sys.argv[1].split("."):
    value = value[part]
if isinstance(value, bool):
    print(str(value).lower())
else:
    print(value)
' "$path"
}

askpass="$work/git-askpass.sh"
cat >"$askpass" <<'ASKPASS_EOF'
#!/usr/bin/env sh
case "${1:-}" in
  *Username*) printf '%s\n' x-access-token ;;
  *Password*) printf '%s\n' "${GH_TOKEN:?}" ;;
  *) exit 1 ;;
esac
ASKPASS_EOF
chmod 700 "$askpass"
export GIT_ASKPASS="$askpass"
export GIT_ASKPASS_REQUIRE=force
export GIT_TERMINAL_PROMPT=0
export GIT_CONFIG_COUNT=1
export GIT_CONFIG_KEY_0=credential.helper
export GIT_CONFIG_VALUE_0=

stage=identity-and-membership
identity="$(api_call GET /user)"
test "$(printf '%s' "$identity" | json_get login)" = ORESoftware
for organization in agent-pontifex fiducia-cloud; do
  membership="$(api_call GET "/user/memberships/orgs/${organization}")"
  test "$(printf '%s' "$membership" | json_get role)" = admin
  test "$(printf '%s' "$membership" | json_get state)" = active
  printf 'membership organization=%s role=admin state=active\n' "$organization"
done

ensure_repo() {
  local organization="$1"
  local name="$2"
  local visibility="$3"
  local description="$4"
  local private_value=false
  test "$visibility" = public || private_value=true

  local existing=''
  if existing="$(api_call GET "/repos/${organization}/${name}" 2>/dev/null)"; then
    test "$(printf '%s' "$existing" | json_get owner.login)" = "$organization"
    if test "$(printf '%s' "$existing" | json_get visibility)" != "$visibility"; then
      printf 'repository repo=%s/%s status=failed reason=visibility-mismatch\n' \
        "$organization" "$name" >&2
      return 70
    fi
    printf 'repository repo=%s/%s action=existing visibility=%s\n' \
      "$organization" "$name" "$visibility"
  else
    body="$(python3 -c '
import json
import sys
print(json.dumps({
    "name": sys.argv[1],
    "description": sys.argv[2],
    "visibility": sys.argv[3],
    "private": sys.argv[4] == "true",
    "has_issues": True,
    "has_projects": True,
    "has_wiki": False,
    "auto_init": False,
}))
' "$name" "$description" "$visibility" "$private_value")"
    printf '%s' "$body" | api_call POST "/orgs/${organization}/repos" >/dev/null
    printf 'repository repo=%s/%s action=created visibility=%s\n' \
      "$organization" "$name" "$visibility"
  fi

  body="$(python3 -c '
import json
import sys
print(json.dumps({
    "description": sys.argv[1],
    "has_issues": True,
    "has_projects": True,
    "has_wiki": False,
    "delete_branch_on_merge": True,
    "allow_squash_merge": True,
    "allow_merge_commit": True,
    "allow_rebase_merge": True,
}))
' "$description")"
  printf '%s' "$body" | api_call PATCH "/repos/${organization}/${name}" >/dev/null
  api_call PUT "/repos/${organization}/${name}/vulnerability-alerts" >/dev/null 2>&1 || true
}

set_topics() {
  local repository="$1"
  shift
  body="$(python3 -c 'import json,sys; print(json.dumps({"names": sys.argv[1:]}))' "$@")"
  printf '%s' "$body" | api_call PUT "/repos/${repository}/topics" >/dev/null
}

stage=create-repositories
ensure_repo agent-pontifex ai-agent-bridge.rs public \
  'Open-source Agent Pontifex topic-routed multi-agent bridge server in Rust.'
ensure_repo agent-pontifex ai-agent-coordinator.rs public \
  'Open-source Agent Pontifex leased-job coordinator and model gateway in Rust.'
ensure_repo agent-pontifex agent-sdk.rs public \
  'Shared Agent Pontifex Rust protocol contracts and typed bridge/coordinator SDK.'
ensure_repo fiducia-cloud fiducia-ai-agent-coordinator.rs private \
  'Fiducia-specific supervised AI-agent coordinator and control plane in Rust.'

set_topics agent-pontifex/ai-agent-bridge.rs rust ai-agents multi-agent websocket agent-pontifex bridge-server
set_topics agent-pontifex/ai-agent-coordinator.rs rust ai-agents multi-agent agent-pontifex coordinator job-queue
set_topics agent-pontifex/agent-sdk.rs rust sdk protocol ai-agents agent-pontifex
set_topics fiducia-cloud/fiducia-ai-agent-coordinator.rs rust ai-agents fiducia coordinator fencing leases

repo_main_sha() {
  local repository="$1"
  local response=''
  if response="$(api_call GET "/repos/${repository}/git/ref/heads/main" 2>/dev/null)"; then
    printf '%s' "$response" | json_get object.sha
  fi
}

set_default_main() {
  local repository="$1"
  printf '%s' '{"default_branch":"main"}' \
    | api_call PATCH "/repos/${repository}" >/dev/null
}

mirror_repo() {
  local source="$1"
  local target="$2"
  local directory="$work/mirror-$(tr '/.' '--' <<<"$target")"

  printf 'mirror source=%s target=%s stage=clone\n' "$source" "$target"
  git clone --bare "https://github.com/${source}.git" "$directory" >/dev/null
  git -C "$directory" remote add target "https://github.com/${target}.git"

  local source_main
  source_main="$(git -C "$directory" rev-parse refs/heads/main)"
  local target_main=''
  target_main="$(repo_main_sha "$target")"
  if test -n "$target_main" && test "$target_main" != "$source_main"; then
    git -C "$directory" fetch target main:refs/remotes/target/main >/dev/null
    if ! git -C "$directory" merge-base --is-ancestor refs/remotes/target/main refs/heads/main; then
      printf 'mirror target=%s status=failed reason=divergent-main target_sha=%s source_sha=%s\n' \
        "$target" "$target_main" "$source_main" >&2
      return 71
    fi
  fi

  git -C "$directory" push target 'refs/heads/*:refs/heads/*' >/dev/null
  if git -C "$directory" show-ref --tags --quiet; then
    git -C "$directory" push target 'refs/tags/*:refs/tags/*' >/dev/null
  fi
  set_default_main "$target"

  local published_main
  published_main="$(repo_main_sha "$target")"
  test "$published_main" = "$source_main"
  printf 'mirror source=%s target=%s status=published main=%s\n' \
    "$source" "$target" "$published_main"
}

stage=publish-community-bridge
mirror_repo ORESoftware/ai-agent-bridge.rs agent-pontifex/ai-agent-bridge.rs

stage=publish-community-coordinator
mirror_repo ORESoftware/ai-agent-coordinator.rs agent-pontifex/ai-agent-coordinator.rs

stage=publish-fiducia-coordinator
mirror_repo fiducia-cloud/fiducia-ai-agent-control-plane fiducia-cloud/fiducia-ai-agent-coordinator.rs

publish_sdk() {
  local source_repo=ORESoftware/ai-agent-bridge.rs
  local source_branch=agent/shared-protocol-sdk-foundation
  local target_repo=agent-pontifex/agent-sdk.rs
  local source_dir="$work/sdk-source"
  local target_dir="$work/sdk-target"

  printf 'sdk stage=clone source=%s branch=%s\n' "$source_repo" "$source_branch"
  git clone "https://github.com/${source_repo}.git" "$source_dir" >/dev/null
  git -C "$source_dir" fetch origin "$source_branch" >/dev/null
  local source_commit
  source_commit="$(git -C "$source_dir" rev-parse "origin/${source_branch}")"
  local split_commit
  split_commit="$(git -C "$source_dir" subtree split --prefix=sdk "origin/${source_branch}")"
  [[ "$source_commit" =~ ^[0-9a-f]{40}$ ]]
  [[ "$split_commit" =~ ^[0-9a-f]{40}$ ]]

  local target_main=''
  target_main="$(repo_main_sha "$target_repo")"
  if test -z "$target_main"; then
    git -C "$source_dir" push "https://github.com/${target_repo}.git" \
      "${split_commit}:refs/heads/main" >/dev/null
  fi
  git -C "$source_dir" push "https://github.com/${target_repo}.git" \
    "${split_commit}:refs/heads/upstream-sdk-extraction" >/dev/null 2>&1 || true

  git clone "https://github.com/${target_repo}.git" "$target_dir" >/dev/null
  git -C "$target_dir" checkout main >/dev/null

  if test -f "$target_dir/.agent-pontifex-source.json"; then
    test "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["source_commit"])' "$target_dir/.agent-pontifex-source.json")" = "$source_commit"
    test "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["split_commit"])' "$target_dir/.agent-pontifex-source.json")" = "$split_commit"
  else
    test "$(git -C "$target_dir" rev-parse HEAD)" = "$split_commit"

    cat >"$target_dir/Cargo.toml" <<'EOF'
[workspace]
resolver = "2"
members = [
    "agent-pontifex-protocol",
    "agent-pontifex-sdk",
]
EOF

    sed -i \
      's#https://github.com/ORESoftware/ai-agent-bridge.rs#https://github.com/agent-pontifex/agent-sdk.rs#g' \
      "$target_dir/agent-pontifex-protocol/Cargo.toml" \
      "$target_dir/agent-pontifex-sdk/Cargo.toml"

    git -C "$source_dir" show "origin/${source_branch}:LICENSE" >"$target_dir/LICENSE"
    cat >"$target_dir/README.md" <<'EOF'
# Agent Pontifex SDK

Shared, vendor-neutral Rust contracts and clients for Agent Pontifex-compatible bridge and coordinator servers.

## Crates

- `agent-pontifex-protocol`: versioned discovery, bridge, presence, messaging, context, repository-path lease, and coordinator-job contracts.
- `agent-pontifex-sdk`: credential-safe typed HTTP clients for bridge and coordinator implementations.

Fiducia-specific authority, tenancy, review, storage, and fencing semantics remain downstream in `fiducia-cloud`; they are advertised through namespaced extensions rather than becoming dependencies of this public SDK.

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo doc --workspace --no-deps
```
EOF

    cat >"$target_dir/SECURITY.md" <<'EOF'
# Security policy

Report vulnerabilities through GitHub private vulnerability reporting for this repository. Do not include credentials, customer data, or active exploit material in public issues.
EOF

    mkdir -p "$target_dir/.github/workflows"
    cat >"$target_dir/.github/workflows/ci.yml" <<'EOF'
name: CI

on:
  pull_request:
  push:
    branches: [main]
  workflow_dispatch:

permissions:
  contents: read

concurrency:
  group: agent-pontifex-sdk-${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

jobs:
  test:
    runs-on: ubuntu-latest
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
      - uses: dtolnay/rust-toolchain@4be7066ada62dd38de10e7b70166bc74ed198c30 # stable
        with:
          toolchain: '1.95.0'
          components: rustfmt, clippy
      - run: cargo fmt --all -- --check
      - run: cargo clippy --workspace --all-targets -- -D warnings
      - run: cargo test --workspace
      - run: cargo doc --workspace --no-deps
EOF

    python3 -c '
import json
import sys
with open(sys.argv[1], "w", encoding="utf-8") as output:
    json.dump({
        "source_repository": sys.argv[2],
        "source_branch": sys.argv[3],
        "source_commit": sys.argv[4],
        "split_commit": sys.argv[5],
    }, output, indent=2)
    output.write("\n")
' "$target_dir/.agent-pontifex-source.json" "$source_repo" "$source_branch" "$source_commit" "$split_commit"

    git -C "$target_dir" config user.name 'Agent Pontifex bootstrap'
    git -C "$target_dir" config user.email '41898282+github-actions[bot]@users.noreply.github.com'
    git -C "$target_dir" add \
      .agent-pontifex-source.json .github Cargo.toml LICENSE README.md SECURITY.md \
      agent-pontifex-protocol/Cargo.toml agent-pontifex-sdk/Cargo.toml
    git -C "$target_dir" commit -m 'chore: establish standalone Agent Pontifex SDK workspace' >/dev/null
    git -C "$target_dir" push origin main >/dev/null
  fi

  set_default_main "$target_repo"
  api_call GET "/repos/${target_repo}/contents/Cargo.toml?ref=main" >/dev/null
  api_call GET "/repos/${target_repo}/contents/agent-pontifex-protocol/src/lib.rs?ref=main" >/dev/null
  api_call GET "/repos/${target_repo}/contents/agent-pontifex-sdk/src/lib.rs?ref=main" >/dev/null
  printf 'sdk target=%s status=published source_commit=%s split_commit=%s main=%s\n' \
    "$target_repo" "$source_commit" "$split_commit" "$(repo_main_sha "$target_repo")"
}

stage=publish-sdk
publish_sdk

stage=final-verification
for repository in \
  agent-pontifex/ai-agent-bridge.rs \
  agent-pontifex/ai-agent-coordinator.rs \
  agent-pontifex/agent-sdk.rs \
  fiducia-cloud/fiducia-ai-agent-coordinator.rs
do
  repo="$(api_call GET "/repos/${repository}")"
  test "$(printf '%s' "$repo" | json_get archived)" = false
  test "$(printf '%s' "$repo" | json_get disabled)" = false
  test "$(printf '%s' "$repo" | json_get default_branch)" = main
  printf 'verified repo=%s visibility=%s default=main\n' \
    "$repository" "$(printf '%s' "$repo" | json_get visibility)"
done

printf 'publisher status=success repositories=4\n'

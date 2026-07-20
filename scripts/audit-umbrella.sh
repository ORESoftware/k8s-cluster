#!/usr/bin/env bash
# Validate the umbrella checkout without building or mutating any child repo.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

fail() {
  printf 'umbrella audit failed: %s\n' "$*" >&2
  exit 1
}

require_tracked_file() {
  local path="$1"
  [[ -f "$path" ]] || fail "missing required file: $path"
  git ls-files --error-unmatch "$path" >/dev/null 2>&1 ||
    fail "required file is not tracked by the umbrella: $path"
}

# These paths document the ownership boundary. The four application trees are
# vendored snapshots; canonical-mcp-server.rs is the umbrella's sole gitlink.
require_tracked_file canonical-monorepo/build.sh
require_tracked_file canonical-web-server.rs/Cargo.toml
require_tracked_file canonical-marketing-site.web/package.json
require_tracked_file canonical-interfaces/package.json
[[ -f canonical-mcp-server.rs/Cargo.toml ]] ||
  fail "MCP submodule is not initialized: canonical-mcp-server.rs/Cargo.toml"

[[ -x build.sh ]] || fail "root build.sh must remain executable"
grep -Fq 'canonical-marketing-site.web' build.sh ||
  fail "root build.sh no longer builds the marketing site"
grep -Fq 'canonical-web-server.rs' build.sh ||
  fail "root build.sh no longer builds the web server"
grep -Fq 'npm ci && npm run typecheck && npm test && npm run build' build.sh ||
  fail "root build.sh no longer verifies the IndexedDB client"
grep -Fq 'cargo build --locked --release --workspace --bins' build.sh ||
  fail "root build.sh must build every locked Rust binary"
grep -Fq 'canonical-web-server serve' build.sh ||
  fail "root build.sh must document the isolated web process"
grep -Fq 'canonical-session-revoker run' build.sh ||
  fail "root build.sh must document the isolated revoker process"

conflict_markers="$(git grep -n -I -E '^(<<<<<<< |[|]{7} |=======|>>>>>>> )' -- . || true)"
[[ -z "$conflict_markers" ]] ||
  fail "unresolved merge markers found:\n$conflict_markers"

# Reject filenames that commonly carry live credentials. Explicit examples and
# direnv loaders are safe documentation/configuration and remain allowed.
suspicious_paths=()
while IFS= read -r path; do
  base="${path##*/}"
  case "$base" in
    .env.example|.env.sample|.env.template|.envrc)
      ;;
    .env|.env.*|.npmrc|credentials.json|service-account.json|id_rsa|id_ed25519)
      suspicious_paths+=("$path")
      ;;
    *.pem|*.p12|*.pfx|*.key)
      suspicious_paths+=("$path")
      ;;
  esac
done < <(git ls-files)

if ((${#suspicious_paths[@]} > 0)); then
  printf -v suspicious_list '  %s\n' "${suspicious_paths[@]}"
  fail "tracked credential-like filenames found:\n$suspicious_list"
fi

# High-confidence token and private-key signatures. Placeholders such as
# SUPABASE_PUBLISHABLE_KEY are intentionally not rejected.
secret_hits="$(git grep -n -I -E \
  '(gh[pousr]_[A-Za-z0-9]{32,}|github_pat_[A-Za-z0-9_]{40,}|AKIA[0-9A-Z]{16}|sbp_[A-Za-z0-9]{32,}|-----BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY-----|eyJ[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{20,})' \
  -- . || true)"
[[ -z "$secret_hits" ]] ||
  fail "high-confidence secret material found in tracked content:\n$secret_hits"

mcp_path="$(git config -f .gitmodules --get submodule.canonical-mcp-server.rs.path || true)"
mcp_url="$(git config -f .gitmodules --get submodule.canonical-mcp-server.rs.url || true)"
mcp_branch="$(git config -f .gitmodules --get submodule.canonical-mcp-server.rs.branch || true)"
[[ "$mcp_path" == "canonical-mcp-server.rs" ]] || fail "unexpected MCP submodule path"
[[ "$mcp_url" == "https://github.com/canonical-cloud/canonical-mcp-server.rs.git" ]] ||
  fail "unexpected MCP submodule URL"
[[ "$mcp_branch" == "main" ]] || fail "MCP submodule must follow main"

submodule_count="$(git config -f .gitmodules --get-regexp '^submodule\..*\.path$' | wc -l | tr -d ' ')"
[[ "$submodule_count" == "1" ]] || fail "the umbrella must have exactly one root submodule"

gitlink="$(git ls-files --stage "$mcp_path")"
read -r mode pinned_sha stage tracked_path <<<"$gitlink"
[[ "$mode" == "160000" && "$stage" == "0" && "$tracked_path" == "$mcp_path" ]] ||
  fail "MCP path is not a stage-zero gitlink"
[[ "$pinned_sha" =~ ^[0-9a-f]{40}$ ]] || fail "invalid MCP gitlink SHA"

checked_out_sha="$(git -C "$mcp_path" rev-parse HEAD 2>/dev/null || true)"
[[ "$checked_out_sha" == "$pinned_sha" ]] ||
  fail "checked-out MCP SHA does not match the umbrella gitlink"

remote_main_sha="$(git ls-remote "$mcp_url" refs/heads/main | awk '{print $1}')"
[[ "$remote_main_sha" =~ ^[0-9a-f]{40}$ ]] || fail "could not resolve MCP main"
[[ "$remote_main_sha" == "$pinned_sha" ]] ||
  fail "MCP gitlink $pinned_sha is behind main $remote_main_sha"

printf 'Umbrella contract verified; MCP is pinned to %s.\n' "$pinned_sha"

#!/usr/bin/env bash
set -euo pipefail

failures=0
allow_dirty=0

fail() {
  echo "FAIL: $*" >&2
  failures=$((failures + 1))
}

warn() {
  echo "WARN: $*" >&2
}

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --allow-dirty)
      allow_dirty=1
      ;;
    -h|--help)
      echo "Usage: scripts/audit-repo-state.sh [--allow-dirty]"
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 64
      ;;
  esac
  shift
done

if [[ ! -f .gitmodules ]]; then
  fail "missing .gitmodules"
fi

declare -a module_names=()
declare -a module_paths=()

while IFS=' ' read -r key module_path; do
  module_name="${key#submodule.}"
  module_name="${module_name%.path}"
  module_names+=("$module_name")
  module_paths+=("$module_path")
done < <(git config -f .gitmodules --get-regexp '^submodule\..*\.path$' || true)

if [[ ${#module_paths[@]} -eq 0 ]]; then
  fail "no submodules declared in .gitmodules"
fi

if [[ -n "$(git status --porcelain=v1)" && "$allow_dirty" -eq 0 ]]; then
  fail "superproject has local changes"
  git status --short >&2
elif [[ -n "$(git status --porcelain=v1)" ]]; then
  warn "superproject has local changes; allowed by --allow-dirty"
fi

if [[ -f .env.example ]]; then
  if git check-ignore -q .env.example; then
    fail ".env.example is ignored; safe env templates must be tracked"
  fi
else
  fail "missing .env.example with placeholder values"
fi

tracked_secret_paths="$(
  git ls-files \
    | grep -E '(^|/)(env/|\.env($|\.)|id_rsa[^/]*$|.*\.(pem|key|p12|pfx|p8)$)' \
    | grep -v -E '(^|/)\.env\.example$' \
    || true
)"
if [[ -n "$tracked_secret_paths" ]]; then
  fail "tracked secret-like paths found"
  printf '%s\n' "$tracked_secret_paths" >&2
fi

# The deploy docs tell operators to keep real values in .env.local; make sure
# an accidental `git add .env.local` can never be committed silently.
if ! git check-ignore -q .env.local; then
  fail ".env.local is not git-ignored; real env files must be unignorable"
fi

scan_git_repo() {
  local repo="$1"
  local label="$2"
  local marker_output
  local secret_output

  marker_output="$(
    git -C "$repo" grep -n -E '^(<<<<<<<|=======|>>>>>>>)' -- . \
      ':(exclude)*.lock' \
      ':(exclude)dist/**' \
      ':(exclude)target/**' \
      ':(exclude)node_modules/**' \
      2>/dev/null || true
  )"
  if [[ -n "$marker_output" ]]; then
    fail "$label has conflict markers"
    printf '%s\n' "$marker_output" >&2
  fi

  # Covers GitHub classic/fine-grained/app tokens, Supabase secret keys,
  # three-segment JWTs (Supabase service-role keys are JWTs), private keys,
  # and AWS access-key IDs.
  secret_output="$(
    git -C "$repo" grep -n -E 'gh[opsu]_[A-Za-z0-9]{36}|github_pat_[A-Za-z0-9_]{22,}|sb_secret_[A-Za-z0-9_-]{10,}|eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}|-----BEGIN [A-Z ]*PRIVATE KEY-----|AKIA[0-9A-Z]{16}' -- . \
      ':(exclude)*.lock' \
      ':(exclude)dist/**' \
      ':(exclude)target/**' \
      ':(exclude)node_modules/**' \
      2>/dev/null || true
  )"
  if [[ -n "$secret_output" ]]; then
    fail "$label has tracked secret-looking values"
    printf '%s\n' "$secret_output" >&2
  fi
}

scan_git_repo "$repo_root" "superproject"

for i in "${!module_paths[@]}"; do
  module_name="${module_names[$i]}"
  module_path="${module_paths[$i]}"
  module_url="$(git config -f .gitmodules --get "submodule.${module_name}.url" || true)"
  module_branch="$(git config -f .gitmodules --get "submodule.${module_name}.branch" || true)"

  if [[ -z "$module_url" ]]; then
    fail "$module_path is missing a submodule URL"
  elif [[ ! "$module_url" =~ ^git@github\.com:canonical-cloud/ ]]; then
    fail "$module_path url is not an SSH canonical-cloud remote: $module_url"
  fi

  if [[ -z "$module_branch" ]]; then
    fail "$module_path is missing a submodule branch"
  elif [[ "$module_branch" != "main" ]]; then
    fail "$module_path submodule branch is '$module_branch', expected 'main'"
  fi

  # The recorded gitlink must be an ancestor of (or equal to) the tracked branch
  # on the remote — i.e. the pin points at real, pushed history, not a local-only
  # or detached commit. Remote verification is a blocking supply-chain gate.
  if [[ -d "$module_path" ]]; then
    pinned_sha="$(git -C "$module_path" rev-parse HEAD 2>/dev/null || true)"
    if git -C "$module_path" fetch -q origin "$module_branch" 2>/dev/null; then
      if [[ -n "$pinned_sha" ]] && ! git -C "$module_path" merge-base --is-ancestor "$pinned_sha" FETCH_HEAD 2>/dev/null; then
        fail "$module_path pin $pinned_sha is not on origin/$module_branch (unpushed or diverged)"
      fi
    else
      fail "$module_path: could not fetch origin/$module_branch to verify the pin"
    fi
  fi

  if [[ ! -d "$module_path" ]]; then
    fail "$module_path is not initialized"
    continue
  fi

  if [[ -n "$(git -C "$module_path" status --porcelain=v1)" ]]; then
    fail "$module_path has local changes"
    git -C "$module_path" status --short >&2
  fi

  if ! grep -q "\`$module_path\`" README.md; then
    fail "README.md app list is missing $module_path"
  fi

  # A submodule is a deployable *service* if it ships a runnable server: a Rust
  # binary crate, or a web app with an Astro build. Library/interface repos (e.g.
  # canonical-interfaces: schema + generated adapters) are not deployed and are
  # exempt from the Dockerfile requirement.
  is_rust_service=0
  is_web_service=0
  if [[ -f "$module_path/Cargo.toml" && ( -f "$module_path/src/main.rs" || -d "$module_path/src/bin" ) ]]; then
    is_rust_service=1
  fi
  if compgen -G "$module_path/astro.config.*" >/dev/null 2>&1; then
    is_web_service=1
  fi

  if [[ "$is_rust_service" -eq 1 || "$is_web_service" -eq 1 ]]; then
    if [[ ! -f "$module_path/Dockerfile" ]]; then
      fail "$module_path (service) is missing Dockerfile"
    fi
    if [[ ! -f "$module_path/.dockerignore" ]]; then
      fail "$module_path (service) is missing .dockerignore"
    fi
  fi

  if [[ "$is_rust_service" -eq 1 && -f "$module_path/Dockerfile" ]]; then
    if ! grep -Eq '^FROM gcr\.io/distroless/cc-debian12:nonroot(@sha256:[0-9a-f]{64})?( AS [A-Za-z0-9_-]+)?$' "$module_path/Dockerfile"; then
      fail "$module_path Rust runtime image is not distroless nonroot"
    fi

    if ! grep -Eq '^COPY --from=[^[:space:]]+ --chown=65532:65532[[:space:]]' "$module_path/Dockerfile" \
      || ! grep -q 'target/release/' "$module_path/Dockerfile"; then
      fail "$module_path does not copy its release binary from a builder as nonroot"
    fi
  fi

  scan_git_repo "$module_path" "$module_path"
done

if [[ ! -f docs/repo-boundaries.md ]]; then
  fail "missing docs/repo-boundaries.md"
fi

if [[ "$failures" -gt 0 ]]; then
  echo
  echo "canonical monorepo audit failed with $failures issue(s)" >&2
  exit 1
fi

echo "canonical monorepo audit passed"

#!/usr/bin/env bash
set -euo pipefail

# Pre-deploy audit of the superproject + every app submodule: fails on dirty
# checkouts, conflict markers, tracked/committed secrets, mutable or fail-open
# workflows, dependency lifecycle hooks, unpinned container bases, unsafe
# runtime identities, missing Docker update automation, unreproducible README
# commands, tracked directories without README entrypoints, readme app-list drift,
# and visibility-policy drift. Rust tool runners
# may use the explicit audited `tool-runner-nonroot` profile when their contract
# requires OS executables.

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

if [[ -f env/.prod.env ]]; then
  warn "local env/.prod.env exists; it must remain ignored and untracked"
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
    | grep -E '(^|/)(env/|\.env($|\.)|.*\.(pem|key|p12|pfx)$)' \
    | grep -v -E '(^|/)\.env\.example$' \
    || true
)"
if [[ -n "$tracked_secret_paths" ]]; then
  fail "tracked secret-like paths found"
  printf '%s\n' "$tracked_secret_paths" >&2
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

  secret_output="$(
    git -C "$repo" grep -n -E 'sb_secret_[A-Za-z0-9_-]{16,}|ghp_[A-Za-z0-9]{36}|^[[:space:]]*-----BEGIN [A-Z ]*PRIVATE KEY-----' -- . \
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

scan_action_pins() {
  local repo="$1"
  local label="$2"
  local action_output

  # Immutable pins come in two shapes: a 40-hex commit SHA for repo actions,
  # and a sha256 content digest for `docker://` actions (a digest is at least
  # as immutable as a commit — the registry resolves it content-addressed).
  action_output="$(
    git -C "$repo" grep -n -E \
      'uses:[[:space:]]*[^[:space:]#]+@[^[:space:]#]+' -- \
      '.github/workflows/*.yml' '.github/workflows/*.yaml' 2>/dev/null \
      | grep -v -E \
        'uses:[[:space:]]*[^[:space:]#]+@[0-9a-f]{40}([[:space:]]|$)|uses:[[:space:]]*docker://[^[:space:]#]+@sha256:[0-9a-f]{64}([[:space:]]|$)' \
      || true
  )"
  if [[ -n "$action_output" ]]; then
    fail "$label has GitHub Actions that are not pinned to full commit SHAs (or docker digests)"
    printf '%s\n' "$action_output" >&2
  fi
}

scan_workflow_hardening() {
  local repo="$1"
  local label="$2"
  local fail_open_output
  local lifecycle_script_output
  local unlocked_cargo_output
  local moving_ref_output
  local workflow
  local runs_on_count
  local timeout_count
  local checkout_count
  local nonpersisting_checkout_count
  local mutation_output
  local -a workflow_files=()

  shopt -s nullglob
  workflow_files=(
    "$repo"/.github/workflows/*.yml
    "$repo"/.github/workflows/*.yaml
  )
  shopt -u nullglob

  if [[ ${#workflow_files[@]} -eq 0 ]]; then
    fail "$label has no executable GitHub Actions workflow"
  fi

  for workflow in "${workflow_files[@]}"; do
    if ! grep -q '^permissions:' "$workflow"; then
      fail "$label workflow ${workflow#$repo/} has no explicit top-level permissions"
    fi
    if ! grep -q '^concurrency:' "$workflow"; then
      fail "$label workflow ${workflow#$repo/} has no concurrency policy"
    fi

    runs_on_count="$(grep -c '^[[:space:]]*runs-on:' "$workflow" || true)"
    timeout_count="$(grep -c '^[[:space:]]*timeout-minutes:' "$workflow" || true)"
    if [[ "$runs_on_count" -ne "$timeout_count" ]]; then
      fail "$label workflow ${workflow#$repo/} does not bound every runner job"
    fi

    checkout_count="$(grep -c 'uses:[[:space:]]*actions/checkout@' "$workflow" || true)"
    nonpersisting_checkout_count="$(grep -c 'persist-credentials:[[:space:]]*false' "$workflow" || true)"
    if [[ "$checkout_count" -ne "$nonpersisting_checkout_count" ]]; then
      fail "$label workflow ${workflow#$repo/} persists checkout credentials"
    fi

    if grep -q -E 'pull_request_target:|workflow_run:|issue_comment:' "$workflow"; then
      fail "$label workflow ${workflow#$repo/} uses a privileged untrusted-code trigger"
    fi
    if grep -q -E 'ghcr\.io/fiducia-cloud/[^[:space:]]+:(latest|main|edge)([[:space:]]|$)' "$workflow"; then
      fail "$label workflow ${workflow#$repo/} publishes or consumes a mutable image tag"
    fi
  done

  if [[ "$label" != "superproject" ]]; then
    mutation_output="$(
      grep -n -E \
        'KUBE_CONFIG|kubectl[[:space:]].*(apply|set[[:space:]]+image)|wrangler.*deploy|npm[[:space:]]+run[[:space:]]+deploy' \
        "${workflow_files[@]}" 2>/dev/null || true
    )"
    if [[ -n "$mutation_output" ]]; then
      fail "$label contains component-repository deployment commands"
      printf '%s\n' "$mutation_output" >&2
    fi
  fi

  scan_action_pins "$repo" "$label"

  fail_open_output="$(
    git -C "$repo" grep -n -E \
      'continue-on-error:[[:space:]]*true|[|][|][[:space:]]*(true|echo)|exit[[:space:]]+0|npm[[:space:]]+install([[:space:]]|$)' -- \
      '.github/workflows/*.yml' '.github/workflows/*.yaml' 2>/dev/null \
      | grep -v -E ':[0-9]+:[[:space:]]*#' \
      || true
  )"
  if [[ -n "$fail_open_output" ]]; then
    fail "$label has fail-open or lockfile-bypassing workflow commands"
    printf '%s\n' "$fail_open_output" >&2
  fi

  lifecycle_script_output="$(
    git -C "$repo" grep -n -E \
      'npm[[:space:]]+ci([[:space:]]|$)' -- \
      '.github/workflows/*.yml' '.github/workflows/*.yaml' 2>/dev/null \
      | grep -v -- '--ignore-scripts' \
      | grep -v -E ':[0-9]+:[[:space:]]*#' \
      || true
  )"
  if [[ -n "$lifecycle_script_output" ]]; then
    fail "$label runs npm dependency lifecycle scripts in a workflow"
    printf '%s\n' "$lifecycle_script_output" >&2
  fi

  unlocked_cargo_output="$(
    git -C "$repo" grep -n -E \
      '(^|[[:space:]])cargo[[:space:]]+(build|check|clippy|run|test)([[:space:]]|$)' -- \
      '.github/workflows/*.yml' '.github/workflows/*.yaml' 2>/dev/null \
      | grep -v -- '--locked' \
      || true
  )"
  if [[ -n "$unlocked_cargo_output" ]]; then
    fail "$label has Cargo workflow commands without --locked"
    printf '%s\n' "$unlocked_cargo_output" >&2
  fi

  moving_ref_output="$(
    git -C "$repo" grep -n -E \
      'ref:[[:space:]]*(main|master|stable)([[:space:]]|$)|^ARG[[:space:]]+[A-Z0-9_]*REF=(main|master|stable)$' -- \
      '.github/workflows/*.yml' '.github/workflows/*.yaml' 'Dockerfile' 2>/dev/null \
      || true
  )"
  if [[ -n "$moving_ref_output" ]]; then
    fail "$label consumes a moving cross-repository revision"
    printf '%s\n' "$moving_ref_output" >&2
  fi
}

scan_container_hardening() {
  local repo="$1"
  local label="$2"
  local dockerfile="$repo/Dockerfile"
  local mutable_bases
  local runtime_user
  local lifecycle_script_output

  if [[ ! -f "$dockerfile" ]]; then
    fail "$label is missing Dockerfile"
    return
  fi

  mutable_bases="$(
    grep -n '^FROM[[:space:]]' "$dockerfile" \
      | grep -v -E '^([0-9]+:)?FROM[[:space:]]+[^[:space:]]+@sha256:[0-9a-f]{64}([[:space:]]+[Aa][Ss][[:space:]]+[A-Za-z0-9._-]+)?$' \
      || true
  )"
  if [[ -n "$mutable_bases" ]]; then
    fail "$label has container base images without immutable sha256 digests"
    printf '%s\n' "$mutable_bases" >&2
  fi

  lifecycle_script_output="$(
    grep -n -E '^RUN[[:space:]].*npm[[:space:]]+ci([[:space:]]|$)' "$dockerfile" \
      | grep -v -- '--ignore-scripts' \
      || true
  )"
  if [[ -n "$lifecycle_script_output" ]]; then
    fail "$label runs npm dependency lifecycle scripts during its container build"
    printf '%s\n' "$lifecycle_script_output" >&2
  fi

  runtime_user="$(awk '/^FROM[[:space:]]/{user=""} /^USER[[:space:]]/{user=$0} END{print user}' "$dockerfile")"
  if [[ -z "$runtime_user" ]]; then
    fail "$label runtime stage has no explicit USER"
  fi

  if [[ ! -f "$repo/.github/dependabot.yml" ]] \
    || ! grep -q 'package-ecosystem:[[:space:]]*docker' "$repo/.github/dependabot.yml"; then
    fail "$label does not track Docker base updates with Dependabot"
  fi
}

scan_readme_reproducibility() {
  local repo="$1"
  local label="$2"
  local unlocked_cargo_output
  local lifecycle_script_output

  unlocked_cargo_output="$(
    git -C "$repo" grep -n -E \
      '^[[:space:]]*([^#[:space:]]+[[:space:]]+)?cargo[[:space:]]+(build|check|clippy|run|test)([[:space:]]|$)' -- \
      '*README*.md' '*readme*.md' \
      ':(exclude)vendor/**' ':(exclude)node_modules/**' 2>/dev/null \
      | grep -v -- '--locked' \
      || true
  )"
  if [[ -n "$unlocked_cargo_output" ]]; then
    fail "$label documents dependency-resolving Cargo commands without --locked"
    printf '%s\n' "$unlocked_cargo_output" >&2
  fi

  lifecycle_script_output="$(
    git -C "$repo" grep -n -E \
      '^[[:space:]]*npm[[:space:]]+ci([[:space:]]|$)' -- \
      '*README*.md' '*readme*.md' \
      ':(exclude)vendor/**' ':(exclude)node_modules/**' 2>/dev/null \
      | grep -v -- '--ignore-scripts' \
      || true
  )"
  if [[ -n "$lifecycle_script_output" ]]; then
    fail "$label documents npm ci without disabling dependency lifecycle scripts"
    printf '%s\n' "$lifecycle_script_output" >&2
  fi
}

scan_directory_readmes() {
  local repo="$1"
  local label="$2"
  local tracked_path dir gitlink skip
  local -a gitlinks=()
  local -a dirs=()

  while IFS= read -r gitlink; do
    [[ -n "$gitlink" ]] && gitlinks+=("$gitlink")
  done < <(git -C "$repo" ls-files --stage | awk '$1 == "160000" { sub(/^[^\t]*\t/, ""); print }')

  while IFS= read -r tracked_path; do
    dir="${tracked_path%/*}"
    [[ "$dir" == "$tracked_path" || "$dir" == "." ]] && continue
    case "/$dir/" in
      */target/*|*/node_modules/*|*/dist/*|*/tmp/*|*/vendor/*) continue ;;
    esac
    if git -C "$repo" check-ignore --no-index -q -- "$dir"; then
      continue
    fi
    skip=0
    # Bash 3.2 (the macOS system Bash) raises an unbound-variable error for an
    # empty array expansion under `set -u`, even when the array was declared.
    if [[ ${#gitlinks[@]} -gt 0 ]]; then
      for gitlink in "${gitlinks[@]}"; do
        if [[ "$dir" == "$gitlink" || "$dir" == "$gitlink/"* ]]; then
          skip=1
          break
        fi
      done
    fi
    [[ "$skip" == 1 ]] && continue
    dirs+=("$dir")
  done < <(git -C "$repo" ls-files)

  [[ ${#dirs[@]} -eq 0 ]] && return
  while IFS= read -r dir; do
    [[ -z "$dir" ]] && continue
    if [[ ! -f "$repo/$dir/README.md" ]]; then
      fail "$label tracked directory '$dir' is missing README.md"
    fi
  done < <(printf '%s\n' "${dirs[@]}" | LC_ALL=C sort -u)
}

scan_git_repo "$repo_root" "superproject"
scan_workflow_hardening "$repo_root" "superproject"
scan_container_hardening "$repo_root" "superproject"
scan_readme_reproducibility "$repo_root" "superproject"
scan_directory_readmes "$repo_root" "superproject"

if git submodule status --recursive | grep -Eq '^[+U-]'; then
  fail "submodule checkout does not exactly match an initialized reviewed gitlink"
fi

for i in "${!module_paths[@]}"; do
  module_name="${module_names[$i]}"
  module_path="${module_paths[$i]}"
  module_url="$(git config -f .gitmodules --get "submodule.${module_name}.url" || true)"
  module_branch="$(git config -f .gitmodules --get "submodule.${module_name}.branch" || true)"

  if [[ -z "$module_url" ]]; then
    fail "$module_path is missing a submodule URL"
  fi

  if [[ "$module_branch" != "main" ]]; then
    fail "$module_path must track main (found '${module_branch:-unset}')"
  fi

  if [[ ! -d "$module_path" ]]; then
    fail "$module_path is not initialized"
    continue
  fi

  if [[ -n "$(git -C "$module_path" status --porcelain=v1)" ]]; then
    fail "$module_path has local changes"
    git -C "$module_path" status --short >&2
  fi

  if ! grep -q "\`$module_path\`" readme.md; then
    fail "readme.md app list is missing $module_path"
  fi

  if [[ ! -f "$module_path/.dockerignore" ]]; then
    fail "$module_path is missing .dockerignore"
  fi

  scan_container_hardening "$module_path" "$module_path"

  if [[ -f "$module_path/Cargo.toml" && ( -f "$module_path/src/main.rs" || -d "$module_path/src/bin" ) ]]; then
    if grep -q -E '^FROM gcr\.io/distroless/cc-debian12:nonroot@sha256:[0-9a-f]{64}$' "$module_path/Dockerfile"; then
      :
    elif grep -q '^LABEL org\.fiducia\.runtime-profile="tool-runner-nonroot"$' "$module_path/Dockerfile"; then
      if ! grep -q '^USER 65532:65532$' "$module_path/Dockerfile"; then
        fail "$module_path tool-runner runtime does not use uid/gid 65532:65532"
      fi
    else
      fail "$module_path Rust runtime is neither distroless nonroot nor an explicit non-root tool runner"
    fi

    if ! grep -q '^COPY --from=build --chown=65532:65532 ' "$module_path/Dockerfile"; then
      fail "$module_path does not copy its release binary with non-root ownership"
    fi
  fi

  scan_git_repo "$module_path" "$module_path"
  scan_workflow_hardening "$module_path" "$module_path"
  scan_readme_reproducibility "$module_path" "$module_path"
  scan_directory_readmes "$module_path" "$module_path"
done

if [[ -f docs/repo-boundaries.md ]]; then
  if command -v gh >/dev/null 2>&1; then
    visibility="$(gh repo view fiducia-cloud/fiducia-monorepo --json visibility --jq .visibility 2>/dev/null || true)"
    if [[ -n "$visibility" && "$visibility" != "PRIVATE" ]]; then
      warn "fiducia-monorepo visibility is $visibility; intended PRIVATE visibility requires an owner decision"
    fi
  else
    warn "gh is unavailable; skipping remote visibility check"
  fi
else
  fail "missing docs/repo-boundaries.md"
fi

if [[ "$failures" -gt 0 ]]; then
  echo
  echo "fiducia monorepo audit failed with $failures issue(s)" >&2
  exit 1
fi

echo "fiducia monorepo audit passed"

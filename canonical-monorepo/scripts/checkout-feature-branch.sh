#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  scripts/checkout-feature-branch.sh <branch> [--base main] [--remote origin] [--dry-run] [--no-fetch] [--set-submodule-branch] [--stage-pins]

Checks out a feature branch across the monorepo and every app submodule:
  - refuses dirty superproject/submodule checkouts
  - checks out the branch in the superproject
  - checks out the branch inside every submodule under .gitmodules
  - creates missing local branches from remote/<branch> when it exists
  - otherwise creates missing branches from remote/<base>

By default this does not push and does not edit .gitmodules. Use
--set-submodule-branch to update .gitmodules branch entries to the feature
branch, and --stage-pins to stage .gitmodules plus gitlink changes.

Examples:
  scripts/checkout-feature-branch.sh feature/customer-streams
  scripts/checkout-feature-branch.sh feature/customer-streams --dry-run
  scripts/checkout-feature-branch.sh feature/customer-streams --set-submodule-branch --stage-pins
USAGE
}

if [[ $# -lt 1 ]]; then
  usage
  exit 64
fi

target_branch="$1"
shift

base_branch="main"
remote="origin"
dry_run=0
fetch=1
set_submodule_branch=0
stage_pins=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --base)
      shift
      if [[ $# -eq 0 ]]; then
        echo "missing value for --base" >&2
        exit 64
      fi
      base_branch="$1"
      ;;
    --remote)
      shift
      if [[ $# -eq 0 ]]; then
        echo "missing value for --remote" >&2
        exit 64
      fi
      remote="$1"
      ;;
    --dry-run)
      dry_run=1
      ;;
    --no-fetch)
      fetch=0
      ;;
    --set-submodule-branch)
      set_submodule_branch=1
      ;;
    --stage-pins)
      stage_pins=1
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 64
      ;;
  esac
  shift
done

validate_branch_name() {
  local branch="$1"
  if [[ ! "$branch" =~ ^[A-Za-z0-9._/-]+$ || "$branch" == -* ]]; then
    echo "invalid branch name: $branch" >&2
    exit 64
  fi
}

# Remote names reach `git fetch`/`ls-remote` as positional arguments; a
# flag-shaped value (e.g. --upload-pack=/attacker) would be parsed as an
# option.
validate_remote_name() {
  local remote="$1"
  if [[ ! "$remote" =~ ^[A-Za-z0-9._/-]+$ || "$remote" == -* ]]; then
    echo "invalid remote name: $remote" >&2
    exit 64
  fi
}

validate_branch_name "$target_branch"
validate_branch_name "$base_branch"
validate_remote_name "$remote"

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

if [[ ! -f .gitmodules ]]; then
  echo "no .gitmodules found in $repo_root" >&2
  exit 1
fi

declare -a module_names=()
declare -a module_paths=()
while IFS=' ' read -r key module_path; do
  module_name="${key#submodule.}"
  module_name="${module_name%.path}"
  module_names+=("$module_name")
  module_paths+=("$module_path")
done < <(git config -f .gitmodules --get-regexp '^submodule\..*\.path$')

if [[ ${#module_paths[@]} -eq 0 ]]; then
  echo "no submodules found in .gitmodules" >&2
  exit 1
fi

remote_branch_exists() {
  local repo="$1"
  local branch="$2"
  git -C "$repo" ls-remote --exit-code --heads "$remote" "$branch" >/dev/null 2>&1
}

fetch_branch_ref() {
  local repo="$1"
  local branch="$2"
  if [[ "$fetch" -eq 1 ]]; then
    git -C "$repo" fetch "$remote" "+refs/heads/${branch}:refs/remotes/${remote}/${branch}" >/dev/null
  fi
}

checkout_or_create_branch() {
  local repo="$1"
  local branch="$2"
  local base="$3"

  if [[ "$fetch" -eq 1 ]]; then
    if remote_branch_exists "$repo" "$branch"; then
      fetch_branch_ref "$repo" "$branch"
    fi
    fetch_branch_ref "$repo" "$base"
  fi

  if git -C "$repo" show-ref --verify --quiet "refs/heads/${branch}"; then
    git -C "$repo" checkout "$branch" >/dev/null
    if git -C "$repo" show-ref --verify --quiet "refs/remotes/${remote}/${branch}"; then
      git -C "$repo" merge --ff-only "${remote}/${branch}" >/dev/null
    fi
    return
  fi

  if git -C "$repo" show-ref --verify --quiet "refs/remotes/${remote}/${branch}"; then
    git -C "$repo" checkout -b "$branch" --track "${remote}/${branch}" >/dev/null
    return
  fi

  if git -C "$repo" show-ref --verify --quiet "refs/remotes/${remote}/${base}"; then
    git -C "$repo" checkout -b "$branch" "${remote}/${base}" >/dev/null
    return
  fi

  if git -C "$repo" show-ref --verify --quiet "refs/heads/${base}"; then
    git -C "$repo" checkout -b "$branch" "$base" >/dev/null
    return
  fi

  echo "could not create $branch in $repo; missing $remote/$base and local $base" >&2
  exit 1
}

if [[ "$dry_run" -eq 0 ]] && [[ -n "$(git status --porcelain=v1)" ]]; then
  echo "refusing to switch branches because the superproject has local changes" >&2
  git status --short >&2
  exit 1
fi

declare -a dirty_modules=()
declare -a missing_base_branches=()

for i in "${!module_paths[@]}"; do
  module_path="${module_paths[$i]}"

  if [[ ! -d "$module_path" ]]; then
    if [[ "$dry_run" -eq 1 ]]; then
      echo "would initialize $module_path"
    else
      git submodule update --init -- "$module_path"
    fi
  fi

  if [[ -d "$module_path" ]] && [[ -n "$(git -C "$module_path" status --porcelain=v1)" ]]; then
    dirty_modules+=("$module_path")
  fi

  if [[ -d "$module_path" ]] && ! remote_branch_exists "$module_path" "$base_branch"; then
    missing_base_branches+=("$module_path")
  fi
done

if [[ ${#dirty_modules[@]} -gt 0 ]]; then
  echo "refusing to switch branches because these submodules have local changes:" >&2
  printf '  %s\n' "${dirty_modules[@]}" >&2
  exit 1
fi

if [[ ${#missing_base_branches[@]} -gt 0 ]]; then
  echo "refusing to switch branches because base '$base_branch' is missing from:" >&2
  printf '  %s\n' "${missing_base_branches[@]}" >&2
  exit 1
fi

if [[ "$dry_run" -eq 1 ]]; then
  echo "would switch superproject to $target_branch from $remote/$base_branch"
else
  checkout_or_create_branch "$repo_root" "$target_branch" "$base_branch"
  echo "switched superproject to $(git branch --show-current)"
fi

for i in "${!module_paths[@]}"; do
  module_name="${module_names[$i]}"
  module_path="${module_paths[$i]}"

  if [[ "$dry_run" -eq 1 ]]; then
    if [[ -d "$module_path" ]] && remote_branch_exists "$module_path" "$target_branch"; then
      echo "would switch $module_path to existing $remote/$target_branch"
    else
      echo "would create $module_path branch $target_branch from $remote/$base_branch"
    fi
    continue
  fi

  checkout_or_create_branch "$module_path" "$target_branch" "$base_branch"

  if [[ "$set_submodule_branch" -eq 1 ]]; then
    git config -f .gitmodules "submodule.${module_name}.branch" "$target_branch"
    git submodule sync -- "$module_path" >/dev/null
  fi

  if [[ "$stage_pins" -eq 1 ]]; then
    git add "$module_path"
  fi

  echo "switched $module_path to $(git -C "$module_path" branch --show-current) at $(git -C "$module_path" rev-parse --short HEAD)"
done

if [[ "$dry_run" -eq 1 ]]; then
  echo "dry run complete; no files changed"
  exit 0
fi

if [[ "$set_submodule_branch" -eq 1 || "$stage_pins" -eq 1 ]]; then
  git add .gitmodules
fi

echo
git submodule status
echo
git status --short

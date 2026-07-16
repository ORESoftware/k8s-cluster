#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  scripts/pin-submodules.sh <branch> [--remote origin] [--dry-run] [--no-fetch]

Pins every submodule declared in .gitmodules to the requested branch:
  - verifies the branch exists on every submodule remote
  - updates each .gitmodules branch entry
  - checks out the branch inside each submodule
  - fast-forwards each submodule to remote/<branch>
  - stages .gitmodules and the resulting gitlink pins

Examples:
  scripts/pin-submodules.sh main
  scripts/pin-submodules.sh dev
  scripts/pin-submodules.sh dev --dry-run
USAGE
}

if [[ $# -lt 1 ]]; then
  usage
  exit 64
fi

target_branch="$1"
shift

remote="origin"
dry_run=0
fetch=1

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run)
      dry_run=1
      ;;
    --no-fetch)
      fetch=0
      ;;
    --remote)
      shift
      if [[ $# -eq 0 ]]; then
        echo "missing value for --remote" >&2
        exit 64
      fi
      remote="$1"
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

if [[ ! "$target_branch" =~ ^[A-Za-z0-9._/-]+$ || "$target_branch" == -* ]]; then
  echo "invalid branch name: $target_branch" >&2
  exit 64
fi

# Remote names reach `git fetch` as positional arguments; a flag-shaped value
# (e.g. --upload-pack=/attacker) would otherwise be parsed as a git option.
if [[ ! "$remote" =~ ^[A-Za-z0-9._/-]+$ || "$remote" == -* ]]; then
  echo "invalid remote name: $remote" >&2
  exit 64
fi

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

declare -a dirty_modules=()
declare -a missing_branches=()

for i in "${!module_paths[@]}"; do
  module_name="${module_names[$i]}"
  module_path="${module_paths[$i]}"
  module_url="$(git config -f .gitmodules --get "submodule.${module_name}.url")"

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

  if ! git ls-remote --exit-code --heads "$module_url" "$target_branch" >/dev/null 2>&1; then
    missing_branches+=("$module_path ($module_url)")
  fi
done

if [[ ${#dirty_modules[@]} -gt 0 ]]; then
  echo "refusing to pin because these submodules have local changes:" >&2
  printf '  %s\n' "${dirty_modules[@]}" >&2
  exit 1
fi

if [[ ${#missing_branches[@]} -gt 0 ]]; then
  echo "refusing to pin because branch '$target_branch' is missing from:" >&2
  printf '  %s\n' "${missing_branches[@]}" >&2
  exit 1
fi

for i in "${!module_paths[@]}"; do
  module_name="${module_names[$i]}"
  module_path="${module_paths[$i]}"

  if [[ "$dry_run" -eq 1 ]]; then
    echo "would pin $module_path to $remote/$target_branch"
    continue
  fi

  git config -f .gitmodules "submodule.${module_name}.branch" "$target_branch"
  git submodule sync -- "$module_path"

  if [[ "$fetch" -eq 1 ]]; then
    git -C "$module_path" fetch "$remote" "$target_branch"
  fi

  if git -C "$module_path" show-ref --verify --quiet "refs/heads/$target_branch"; then
    git -C "$module_path" checkout "$target_branch"
    git -C "$module_path" merge --ff-only "$remote/$target_branch"
  else
    git -C "$module_path" checkout -b "$target_branch" --track "$remote/$target_branch"
  fi

  git add .gitmodules "$module_path"
  echo "pinned $module_path to $(git -C "$module_path" rev-parse --short HEAD) on $target_branch"
done

if [[ "$dry_run" -eq 1 ]]; then
  echo "dry run complete; no files changed"
else
  echo
  git submodule status
  echo
  echo "Pins staged. Review with: git status && git diff --cached --submodule"
fi

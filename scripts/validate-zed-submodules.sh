#!/usr/bin/env bash
set -Eeuo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

[[ -f .zpkg.toml ]] || { echo 'missing .zpkg.toml' >&2; exit 1; }
[[ -f .gitmodules ]] || { echo 'missing .gitmodules' >&2; exit 1; }

dependencies="$({ awk '
  /^[[:space:]]*\[dependencies\][[:space:]]*$/ { in_deps=1; next }
  /^[[:space:]]*\[/ { in_deps=0 }
  in_deps {
    line=$0
    sub(/^[[:space:]]*"/, "", line)
    if (line != $0) { sub(/".*/, "", line); print tolower(line) }
  }
' .zpkg.toml; } | sort -u)"

if grep -Eq '(^|/)[^[:space:]]*(-infra|-cli)$' <<<"$dependencies"; then
  echo 'monorepo Zed dependencies must not import *-infra or *-cli' >&2
  exit 1
fi

normalize_github_repo() {
  local value="${1%/}"
  value="${value%.git}"
  case "$value" in
    https://github.com/*) value="${value#https://github.com/}" ;;
    http://github.com/*) value="${value#http://github.com/}" ;;
    git://github.com/*) value="${value#git://github.com/}" ;;
    git@github.com:*) value="${value#git@github.com:}" ;;
    ssh://git@github.com/*) value="${value#ssh://git@github.com/}" ;;
    github.com/*) value="${value#github.com/}" ;;
    *) return 1 ;;
  esac
  [[ "$value" == */* && "$value" != */*/* ]] || return 1
  printf '%s\n' "${value,,}"
}

expected_paths=$'apps/zed-api-server.rs\napps/zed-clients\napps/zed-docs\napps/zed-e2e\napps/zed-interfaces\napps/zed-pkg.github.io\napps/zed-sync\napps/zed-web-server.rs'
actual_paths="$(git config -f .gitmodules --get-regexp '^submodule\..*\.path$' | awk '{print $2}' | sort -u)"
[[ "$actual_paths" == "$expected_paths" ]] || {
  echo 'unexpected Git-submodule inventory' >&2
  diff -u <(printf '%s\n' "$expected_paths") <(printf '%s\n' "$actual_paths") >&2 || true
  exit 1
}

while read -r _key path; do
  [[ "$path" == apps/* ]] || { printf 'submodule outside apps/: %s\n' "$path" >&2; exit 1; }
  mode="$(git ls-files --stage -- "$path" | awk '{print $1}')"
  [[ "$mode" == 160000 ]] || { printf 'submodule path is not a gitlink: %s\n' "$path" >&2; exit 1; }
done < <(git config -f .gitmodules --get-regexp '^submodule\..*\.path$')

while read -r key url; do
  identity="$(normalize_github_repo "$url" 2>/dev/null || true)"
  [[ -n "$identity" ]] || { printf 'unsupported submodule URL: %s\n' "$url" >&2; exit 1; }
  [[ "$identity" == zed-pkg/* ]] || { printf 'submodule outside zed-pkg: %s\n' "$identity" >&2; exit 1; }
  if grep -Fxq "$identity" <<<"$dependencies"; then
    printf '%s is represented by both Zed and a Git submodule (%s)\n' "$identity" "$key" >&2
    exit 1
  fi
  repo_name="${identity##*/}"
  if [[ "$repo_name" == *-infra || "$repo_name" == *-cli ]]; then
    printf 'monorepo must not import CLI/infra submodule: %s\n' "$identity" >&2
    exit 1
  fi
done < <(git config -f .gitmodules --get-regexp '^submodule\..*\.url$')

path_count="$(printf '%s\n' "$actual_paths" | sed '/^$/d' | wc -l | tr -d ' ')"
branch_count="$(git config -f .gitmodules --get-regexp '^submodule\..*\.branch$' | wc -l | tr -d ' ')"
[[ "$branch_count" == "$path_count" ]] || { echo 'each submodule must declare branch = main metadata' >&2; exit 1; }
if git config -f .gitmodules --get-regexp '^submodule\..*\.branch$' | awk '$2 != "main" { bad=1 } END { exit bad ? 0 : 1 }'; then
  echo 'all submodule branch metadata must be main' >&2
  exit 1
fi

git submodule sync --recursive
git submodule status --recursive >/dev/null
printf 'zed-monorepo Zed/submodule ownership contract validated\n'

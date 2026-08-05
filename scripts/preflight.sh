#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT_DIR"

MODE=${1:-full}
FAILURES=0

pass() { printf 'ok: %s\n' "$1"; }
fail() { printf 'error: %s\n' "$1" >&2; FAILURES=$((FAILURES + 1)); }
optional() { printf 'optional: %s\n' "$1"; }

require_contract_line() {
  local pattern=$1
  local description=$2
  if grep -Eq "$pattern" mise.toml; then
    pass "$description"
  else
    fail "$description is missing from mise.toml"
  fi
}

require_contract_line '^rust = "1\.88\.0"$' 'Rust 1.88.0 contract'
require_contract_line '^node = "22"$' 'Node.js 22 contract'
require_contract_line '^java = "temurin-17"$' 'Temurin Java 17 contract'
require_contract_line '^python = "3\.12"$' 'Python 3.12 contract'
require_contract_line '^dart = "3\.12\.2"$' 'Dart 3.12.2 contract'
require_contract_line '^helm = "3\.17\.3"$' 'Helm 3.17.3 contract'
require_contract_line '^CLIPTOWN_ANDROID_API = "35"$' 'Android API 35 contract'

EXPECTED_SUBMODULE_BRANCH_COUNT=5
SUBMODULE_BRANCH_COUNT=$(git config --file .gitmodules --get-regexp '^submodule\..*\.branch$' 2>/dev/null | wc -l | tr -d ' ')
if [[ "$SUBMODULE_BRANCH_COUNT" == "$EXPECTED_SUBMODULE_BRANCH_COUNT" ]]; then
  pass "$EXPECTED_SUBMODULE_BRANCH_COUNT ClipTown source submodules declare a branch"
else
  fail "expected $EXPECTED_SUBMODULE_BRANCH_COUNT source submodule branch declarations, found $SUBMODULE_BRANCH_COUNT"
fi

for forbidden in apps/cliptown-cli apps/cliptown-infra; do
  if git config --file .gitmodules --get-regexp '^submodule\..*\.path$' 2>/dev/null | awk '{print $2}' | grep -Fxq "$forbidden"; then
    fail "forbidden monorepo submodule is declared: $forbidden"
  elif git ls-files --stage -- "$forbidden" | awk '$1 == "160000" {found=1} END {exit !found}'; then
    fail "forbidden monorepo gitlink is indexed: $forbidden"
  else
    pass "$forbidden remains independently owned"
  fi
done

if git config --file .gitmodules --get-regexp '^submodule\..*\.branch$' 2>/dev/null | grep -v ' main$' >/dev/null; then
  fail 'every ClipTown source submodule must track main'
else
  pass 'every ClipTown source submodule tracks main'
fi

if [[ "$MODE" == '--contract-only' ]]; then
  (( FAILURES == 0 )) || exit 1
  exit 0
fi

check_version() {
  local description=$1
  local expected=$2
  shift 2
  local output
  if ! output=$("$@" 2>&1); then
    fail "$description is unavailable"
    return
  fi
  if grep -Eq "$expected" <<<"$output"; then
    pass "$description: ${output%%$'\n'*}"
  else
    fail "$description does not match the workspace contract: ${output%%$'\n'*}"
  fi
}

if command -v git >/dev/null 2>&1; then
  pass "Git: $(git --version)"
else
  fail 'Git is unavailable'
fi

check_version 'Rust' '^rustc 1\.88\.' rustc --version
check_version 'Node.js' '^v22\.' node --version
check_version 'Dart' '^Dart SDK version: 3\.12\.2([[:space:]]|$)' dart --version
check_version 'Java' 'version "17\.' java -version
check_version 'Python' '^Python 3\.12\.' python3 --version
check_version 'Helm' '^v3\.17\.3([+-]|$)' helm version --short

for tool in flutter buf kubectl kustomize psql; do
  if command -v "$tool" >/dev/null 2>&1; then
    pass "$tool is available"
  else
    optional "$tool is not installed; only its repository-specific checks will be unavailable"
  fi
done

SUBMODULE_STATUS=$(git submodule status --recursive 2>/dev/null || true)
if grep -Eq '^[+-]' <<<"$SUBMODULE_STATUS"; then
  fail 'submodules are missing or do not match recorded gitlinks; rerun bootstrap'
else
  pass 'initialized submodules match recorded gitlinks'
fi

case "$(uname -s)" in
  Darwin)
    optional 'macOS host detected: iOS/macOS simulator checks are supported when Xcode is installed'
    ;;
  Linux)
    optional 'Linux host detected: iOS/macOS checks remain delegated to hosted macOS runners'
    ;;
  MINGW*|MSYS*|CYGWIN*)
    optional 'Windows host detected: native Windows checks are supported; Unix shell checks use Git Bash'
    ;;
  *)
    optional "unrecognized host $(uname -s); platform-only checks may need a hosted runner"
    ;;
esac

optional 'signing identities, app-store credentials, production secrets, and cluster credentials are not required by preflight'

(( FAILURES == 0 )) || exit 1

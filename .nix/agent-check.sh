#!/usr/bin/env bash
set -euo pipefail

export CI="${CI:-1}"
export NO_COLOR="${NO_COLOR:-1}"
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

cache_root="${NIX_AGENT_CACHE_ROOT:-$repo_root/.cache/nix-agent}"
export HOME="$cache_root/home"
export CARGO_HOME="${CARGO_HOME:-$cache_root/cargo-home}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$cache_root/target}"
export GIT_CONFIG_GLOBAL=/dev/null
export GIT_CONFIG_NOSYSTEM=1
export GIT_TERMINAL_PROMPT=0
export XDG_CACHE_HOME="${XDG_CACHE_HOME:-$cache_root/xdg}"
unset GIT_ASKPASS GIT_CONFIG_COUNT GIT_SSH GIT_SSH_COMMAND SSH_ASKPASS SSH_AUTH_SOCK
mkdir -p "$HOME" "$CARGO_HOME" "$CARGO_TARGET_DIR" "$XDG_CACHE_HOME"

require_rust_toolchain() {
  local rustc_version
  local cargo_version

  rustc_version="$(rustc --version)"
  cargo_version="$(cargo --version)"

  case "$rustc_version" in
    "rustc 1.88.0 "*) ;;
    *)
      printf 'expected flake-pinned rustc 1.88.0, found: %s\n' "$rustc_version" >&2
      return 69
      ;;
  esac
  case "$cargo_version" in
    "cargo 1.88.0 "*) ;;
    *)
      printf 'expected flake-pinned cargo 1.88.0, found: %s\n' "$cargo_version" >&2
      return 69
      ;;
  esac

  command -v clippy-driver >/dev/null 2>&1 || {
    printf '%s\n' 'clippy-driver is missing from the flake-pinned Rust toolchain' >&2
    return 69
  }
  command -v rustfmt >/dev/null 2>&1 || {
    printf '%s\n' 'rustfmt is missing from the flake-pinned Rust toolchain' >&2
    return 69
  }

  printf '%s\n%s\n' "$rustc_version" "$cargo_version" >&2
}

require_cargo_audit() {
  if ! command -v cargo-audit >/dev/null 2>&1; then
    printf '%s\n' 'cargo-audit is missing from the flake-pinned Nix shell' >&2
    return 69
  fi
  cargo audit --version >&2
}

verify_rsa_exception() {
  local rsa_tree

  if ! rsa_tree="$(cargo tree --locked --all-features --target all -i rsa 2>/dev/null)"; then
    printf '%s\n' 'could not inspect the resolved dependency graph for rsa' >&2
    return 1
  fi
  if [[ -n "$rsa_tree" ]]; then
    printf '%s\n%s\n' \
      'RUSTSEC-2023-0071 may only be ignored while rsa is inactive lockfile metadata:' \
      "$rsa_tree" >&2
    return 1
  fi

  if ! cargo metadata --locked --format-version 1 |
    jq -e '
      (.packages
        | map(select(.name == "rsa" and .version == "0.9.10"))
        | map(.id)) as $rsa_ids
      | .resolve.root as $root_id
      | [.resolve.nodes[] as $node
          | $node.deps[]
          | select(.pkg == $rsa_ids[0])
          | $node.id] as $parents
      | [.resolve.nodes[]
          | select(.id == $root_id)
          | .deps[].pkg
          | select(test("#(rsa|sqlx-mysql)@"))] as $direct_dependencies
      | ($rsa_ids | length == 1)
        and ($parents | length == 1)
        and ($parents[0] | contains("#sqlx-mysql@"))
        and ($direct_dependencies | length == 0)
    ' >/dev/null; then
    printf '%s\n' \
      'the DEN-538 rsa exception is no longer limited to sqlx-mysql lockfile metadata' >&2
    return 1
  fi

  printf '%s\n' \
    'verified DEN-538: rsa 0.9.10 is inactive and referenced only by sqlx-mysql lockfile metadata' >&2
}

prepare_audit() {
  require_cargo_audit
  cargo fetch --locked
  verify_rsa_exception
}

run_stage() {
  local stage="$1"

  printf '\n==> agent-check stage: %s\n' "$stage" >&2
  require_rust_toolchain
  case "$stage" in
    preflight)
      git diff --check HEAD
      nixfmt --check flake.nix .nix/devshell.nix
      shellcheck .nix/agent-check.sh
      shfmt -i 2 -ci -d .nix/agent-check.sh
      actionlint
      nix flake check --no-update-lock-file --show-trace
      prepare_audit
      ;;
    fmt)
      cargo fmt --all --check
      ;;
    check)
      cargo check --locked --all-targets --all-features
      ;;
    clippy)
      cargo clippy --locked --all-targets --all-features -- -D warnings
      ;;
    test)
      cargo test --locked --all-features
      ;;
    audit-prepare)
      prepare_audit
      ;;
    audit)
      prepare_audit
      cargo audit --deny warnings
      ;;
    audit-json)
      prepare_audit
      cargo audit --deny warnings --json
      ;;
    *)
      printf 'unknown agent-check stage: %s\n' "$stage" >&2
      return 64
      ;;
  esac
}

case "${1:-all}" in
  all)
    for stage in preflight fmt check clippy test audit; do
      run_stage "$stage"
    done
    ;;
  preflight | fmt | check | clippy | test | audit-prepare | audit | audit-json)
    run_stage "$1"
    ;;
  *)
    printf 'usage: %s [all|preflight|fmt|check|clippy|test|audit-prepare|audit|audit-json]\n' "$0" >&2
    exit 64
    ;;
esac

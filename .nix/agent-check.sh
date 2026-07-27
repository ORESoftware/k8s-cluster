#!/usr/bin/env bash
set -euo pipefail

export CI="${CI:-1}"
export NO_COLOR="${NO_COLOR:-1}"
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

cache_root="${NIX_AGENT_CACHE_ROOT:-$repo_root/.cache/nix-agent}"
export CARGO_HOME="${CARGO_HOME:-$cache_root/cargo-home}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$cache_root/target}"
export XDG_CACHE_HOME="${XDG_CACHE_HOME:-$cache_root/xdg}"
mkdir -p "$CARGO_HOME" "$CARGO_TARGET_DIR" "$XDG_CACHE_HOME"

run_stage() {
  local stage="$1"

  printf '\n==> agent-check stage: %s\n' "$stage"
  case "$stage" in
    preflight)
      git diff --check
      nixfmt --check flake.nix .nix/devshell.nix
      shellcheck .nix/agent-check.sh
      shfmt -d .nix/agent-check.sh
      actionlint .github/workflows/ci.yml .github/workflows/nix.yml
      nix flake check --show-trace
      rustc --version
      cargo --version
      ;;
    fmt)
      cargo fmt --all --check
      ;;
    check)
      cargo check --locked --all-targets --all-features
      cargo clippy --locked --all-targets --all-features -- -D warnings
      ;;
    test)
      cargo test --locked --all-features
      ;;
    audit)
      cargo audit
      ;;
    *)
      printf 'unknown agent-check stage: %s\n' "$stage" >&2
      return 64
      ;;
  esac
}

case "${1:-all}" in
  all)
    for stage in preflight fmt check test audit; do
      run_stage "$stage"
    done
    ;;
  preflight | fmt | check | test | audit)
    run_stage "$1"
    ;;
  *)
    printf 'usage: %s [all|preflight|fmt|check|test|audit]\n' "$0" >&2
    exit 64
    ;;
esac

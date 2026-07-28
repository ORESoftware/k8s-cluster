#!/usr/bin/env bash
set -euo pipefail

export CI="${CI:-1}"
export NO_COLOR="${NO_COLOR:-1}"
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

cache_root="${NIX_AGENT_CACHE_ROOT:-$repo_root/.cache/nix-agent}"
rust_toolchain="1.88.0"
export CARGO_HOME="${CARGO_HOME:-$cache_root/cargo-home}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$cache_root/target}"
export RUSTUP_HOME="${RUSTUP_HOME:-$cache_root/rustup}"
export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-$rust_toolchain}"
export XDG_CACHE_HOME="${XDG_CACHE_HOME:-$cache_root/xdg}"
mkdir -p "$CARGO_HOME" "$CARGO_TARGET_DIR" "$RUSTUP_HOME" "$XDG_CACHE_HOME"

ensure_rust_toolchain() {
  if ! rustup toolchain list | grep -Eq '^1\.88\.0(-|[[:space:]])'; then
    rustup toolchain install "$rust_toolchain" --profile minimal
  fi
  rustup component add --toolchain "$rust_toolchain" clippy rustfmt
}

require_cargo_audit() {
  if ! command -v cargo-audit >/dev/null 2>&1; then
    printf '%s\n' 'cargo-audit is missing from the flake-pinned Nix shell' >&2
    return 69
  fi
  cargo audit --version
}

run_stage() {
  local stage="$1"

  printf '\n==> agent-check stage: %s\n' "$stage" >&2
  ensure_rust_toolchain
  case "$stage" in
    preflight)
      git diff --check
      nixfmt --check flake.nix .nix/devshell.nix
      shellcheck .nix/agent-check.sh
      shfmt -i 2 -ci -d .nix/agent-check.sh
      actionlint .github/workflows/ci.yml .github/workflows/nix.yml
      nix flake check --show-trace
      rustc --version
      cargo --version
      require_cargo_audit
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
      require_cargo_audit
      ;;
    audit)
      require_cargo_audit
      cargo audit
      ;;
    audit-json)
      require_cargo_audit >/dev/null
      cargo audit --json
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

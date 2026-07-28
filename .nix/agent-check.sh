#!/usr/bin/env bash
set -euo pipefail

export CI="${CI:-1}"
export NO_COLOR="${NO_COLOR:-1}"
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

cache_root="${NIX_AGENT_CACHE_ROOT:-$repo_root/.cache/nix-agent}"
rust_toolchain="1.88.0"
cargo_audit_version="0.22.2"
export CARGO_HOME="${CARGO_HOME:-$cache_root/cargo-home}"
export CARGO_INSTALL_ROOT="${CARGO_INSTALL_ROOT:-$cache_root/cargo-tools}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$cache_root/target}"
export RUSTUP_HOME="${RUSTUP_HOME:-$cache_root/rustup}"
export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-$rust_toolchain}"
export XDG_CACHE_HOME="${XDG_CACHE_HOME:-$cache_root/xdg}"
export PATH="$CARGO_INSTALL_ROOT/bin:$PATH"
mkdir -p \
  "$CARGO_HOME" \
  "$CARGO_INSTALL_ROOT" \
  "$CARGO_TARGET_DIR" \
  "$RUSTUP_HOME" \
  "$XDG_CACHE_HOME"

ensure_rust_toolchain() {
  if ! rustup toolchain list | grep -Eq '^1\.88\.0(-|[[:space:]])'; then
    rustup toolchain install "$rust_toolchain" --profile minimal
  fi
  rustup component add --toolchain "$rust_toolchain" clippy rustfmt
}

installed_cargo_audit_version() {
  cargo audit --version 2>/dev/null || true
}

prepare_cargo_audit() {
  local installed_version
  installed_version="$(installed_cargo_audit_version)"
  if [[ "$installed_version" != "cargo-audit $cargo_audit_version" ]]; then
    cargo install cargo-audit \
      --version "$cargo_audit_version" \
      --locked \
      --root "$CARGO_INSTALL_ROOT"
  fi
  require_cargo_audit
}

require_cargo_audit() {
  local installed_version
  installed_version="$(installed_cargo_audit_version)"
  if [[ "$installed_version" != "cargo-audit $cargo_audit_version" ]]; then
    printf 'cargo-audit %s is required, found: %s\n' \
      "$cargo_audit_version" \
      "${installed_version:-not installed}" >&2
    return 69
  fi
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
      prepare_cargo_audit
      cargo audit --version
      ;;
    audit)
      prepare_cargo_audit
      cargo audit
      ;;
    audit-json)
      require_cargo_audit
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

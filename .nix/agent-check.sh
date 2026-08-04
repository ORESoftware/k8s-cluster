#!/usr/bin/env bash
set -euo pipefail

export CI="${CI:-1}"
export NO_COLOR="${NO_COLOR:-1}"
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"

readonly INACTIVE_RKYV_ADVISORY="RUSTSEC-2026-0235"
readonly INACTIVE_RKYV_PACKAGE="rkyv@0.7.46"

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
    "rustc 1.95.0 "*) ;;
    *)
      printf 'expected flake-pinned rustc 1.95.0, found: %s\n' "$rustc_version" >&2
      return 69
      ;;
  esac
  case "$cargo_version" in
    "cargo 1.95.0 "*) ;;
    *)
      printf 'expected flake-pinned cargo 1.95.0, found: %s\n' "$cargo_version" >&2
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

verify_postgres_only_orm_graph() {
  if ! cargo metadata --locked --format-version 1 |
    jq -e '
      [.packages[] | select(.name == "rsa")] as $forbidden
      | .resolve.root as $root_id
      | [.resolve.nodes[]
          | select(.id == $root_id)
          | .deps[].pkg
          | select(test("#(rsa|sqlx-mysql)@"))] as $direct_dependencies
      | ($forbidden | length == 0)
        and ($direct_dependencies | length == 0)
    ' >/dev/null; then
    printf '%s\n' \
      'PostgreSQL-only ORM graph unexpectedly contains rsa or sqlx-mysql' >&2
    return 1
  fi

  if cargo tree --locked --all-features --target all -i rsa 2>/dev/null |
    grep -q .; then
    printf '%s\n' 'rsa unexpectedly resolves in the all-feature graph' >&2
    return 1
  fi

  if cargo tree --locked --all-features --target all -i sqlx-mysql 2>/dev/null |
    grep -q .; then
    printf '%s\n' 'sqlx-mysql unexpectedly resolves in the all-feature graph' >&2
    return 1
  fi

  printf '%s\n' \
    'verified DEN-538: rsa is absent and sqlx-mysql is not active' >&2
}

verify_inactive_rkyv_advisory() {
  # SeaORM 2.0's published package metadata records its optional Decimal edge
  # in Cargo.lock, so cargo-audit sees rkyv 0.7.46 even though this application
  # disables SeaORM defaults and does not enable with-rust_decimal. The exact
  # advisory may be ignored only after proving the vulnerable crate is absent
  # from every feature and target in the executable graph.
  if cargo tree --locked --all-features --target all \
    -i "$INACTIVE_RKYV_PACKAGE" 2>/dev/null | grep -q .; then
    printf '%s\n' \
      "$INACTIVE_RKYV_PACKAGE became active; remove the $INACTIVE_RKYV_ADVISORY exception and upgrade the reachable dependency" >&2
    return 1
  fi

  printf '%s\n' \
    "verified DEN-1771: $INACTIVE_RKYV_PACKAGE is lockfile-only and absent from the all-feature graph" >&2
}

prepare_audit() {
  require_cargo_audit
  cargo fetch --locked
  verify_postgres_only_orm_graph
  verify_inactive_rkyv_advisory
}

run_cargo_audit() {
  cargo audit --deny warnings --ignore "$INACTIVE_RKYV_ADVISORY" "$@"
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
      run_cargo_audit
      ;;
    audit-json)
      prepare_audit
      run_cargo_audit --json
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

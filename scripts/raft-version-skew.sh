#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
cd "$repo_root"

old_ref="${LMX_RAFT_OLD_REF:-HEAD~1}"
work_dir="${LMX_RAFT_VERSION_SKEW_DIR:-target/raft-version-skew}"
build_profile="${LMX_RAFT_VERSION_SKEW_PROFILE:-debug}"
test_threads="${TEST_THREADS:-1}"
build_locked="${LMX_RAFT_VERSION_SKEW_LOCKED:-false}"
old_worktree="$work_dir/old-src"
old_target="$work_dir/old-target"

usage() {
  cat >&2 <<EOF
usage: LMX_RAFT_OLD_REF=HEAD~1 $0

Builds an older BrokerRaft binary from a git ref in a temporary worktree,
builds the current binary, then runs the ignored mixed-binary Raft rolling
upgrade smoke.

env:
  LMX_RAFT_OLD_REF=$old_ref
  LMX_RAFT_VERSION_SKEW_DIR=$work_dir
  LMX_RAFT_VERSION_SKEW_PROFILE=$build_profile
  LMX_RAFT_VERSION_SKEW_LOCKED=$build_locked
  LMX_RAFT_NEW_BIN=${LMX_RAFT_NEW_BIN:-}
  TEST_THREADS=$test_threads
EOF
}

normalize_bool() {
  local name="$1"
  local value="$2"
  case "$value" in
    true | false) printf '%s\n' "$value" ;;
    1) printf 'true\n' ;;
    0) printf 'false\n' ;;
    *)
      echo "$name must be true or false; got $value" >&2
      exit 2
      ;;
  esac
}

run() {
  printf '\n==> %s\n' "$*" >&2
  "$@"
}

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
  usage
  exit 0
fi

case "$build_profile" in
  debug | release) ;;
  *)
    echo "LMX_RAFT_VERSION_SKEW_PROFILE must be debug or release; got $build_profile" >&2
    exit 2
    ;;
esac

build_locked="$(normalize_bool LMX_RAFT_VERSION_SKEW_LOCKED "$build_locked")"
if ! [[ "$test_threads" =~ ^[1-9][0-9]*$ ]]; then
  echo "TEST_THREADS must be a positive integer; got $test_threads" >&2
  exit 2
fi

mkdir -p "$work_dir"
if [ -d "$old_worktree/.git" ]; then
  run git worktree remove --force "$old_worktree"
elif [ -e "$old_worktree" ]; then
  rm -rf "$old_worktree"
fi

run git worktree add --detach "$old_worktree" "$old_ref"
trap 'git worktree remove --force "$old_worktree" >/dev/null 2>&1 || true' EXIT

build_args=(build --no-default-features --bin dd-rust-network-mutex)
if [ "$build_locked" = true ]; then
  build_args+=(--locked)
fi
if [ "$build_profile" = release ]; then
  build_args+=(--release)
fi

run env CARGO_TARGET_DIR="$old_target" cargo "${build_args[@]}" --manifest-path "$old_worktree/Cargo.toml"
old_bin="$old_target/$build_profile/dd-rust-network-mutex"
if [ ! -x "$old_bin" ]; then
  echo "old BrokerRaft binary was not built at $old_bin" >&2
  exit 1
fi

if [ -n "${LMX_RAFT_NEW_BIN:-}" ]; then
  new_bin="$LMX_RAFT_NEW_BIN"
else
  run cargo "${build_args[@]}"
  new_bin="$repo_root/target/$build_profile/dd-rust-network-mutex"
fi
if [ ! -x "$new_bin" ]; then
  echo "new BrokerRaft binary is not executable at $new_bin" >&2
  exit 1
fi

run env \
  LMX_RAFT_VERSION_SKEW=1 \
  LMX_RAFT_OLD_BIN="$old_bin" \
  LMX_RAFT_NEW_BIN="$new_bin" \
  cargo test --no-default-features --test raft_version_skew -- --ignored --nocapture --test-threads="$test_threads"

#!/usr/bin/env bash
set -euo pipefail

mode="${1:-sample}"
test_bin="${TEST_BIN:-raft_cluster}"
test_filter="${TEST_FILTER:-raft_lb_seeded_lock_model_fuzz}"
profile="${PROFILE:-profiling}"
out_dir="${OUT_DIR:-target/profiles}"
sample_seconds="${SAMPLE_SECONDS:-8}"
sample_interval_ms="${SAMPLE_INTERVAL_MS:-1}"
perf_freq="${PERF_FREQ:-997}"

mkdir -p "$out_dir"

export RUSTFLAGS="${RUSTFLAGS:-} -C force-frame-pointers=yes"

build_test_binary() {
  cargo test --no-default-features --profile "$profile" --test "$test_bin" --no-run >/dev/null
  find "target/$profile/deps" -maxdepth 1 -type f -perm -111 -name "$test_bin-*" | head -1
}

case "$mode" in
  sample)
    if ! command -v sample >/dev/null 2>&1; then
      echo "Apple sample(1) is not available on this host" >&2
      exit 1
    fi
    bin="$(build_test_binary)"
    out="$out_dir/$test_bin-$test_filter.sample.txt"
    rm -f "$out"
    "$bin" "$test_filter" --nocapture &
    pid=$!
    sample_status=0
    sample "$pid" "$sample_seconds" "$sample_interval_ms" -mayDie -file "$out" || sample_status=$?
    if [ "$sample_status" -ne 0 ]; then
      kill "$pid" >/dev/null 2>&1 || true
      wait "$pid" >/dev/null 2>&1 || true
      echo "sample failed with status $sample_status; macOS may require sudo or full Xcode/Instruments permissions" >&2
      exit "$sample_status"
    fi
    wait "$pid"
    echo "$out"
    ;;
  perf)
    if ! command -v perf >/dev/null 2>&1; then
      echo "Linux perf is not available on this host" >&2
      exit 1
    fi
    bin="$(build_test_binary)"
    out="$out_dir/$test_bin-$test_filter.perf.data"
    perf record -F "$perf_freq" -g -o "$out" -- "$bin" "$test_filter" --nocapture
    perf report -i "$out" --stdio | tee "$out.report.txt"
    echo "$out"
    ;;
  flamegraph)
    if ! cargo flamegraph --help >/dev/null 2>&1; then
      echo "cargo-flamegraph is not installed. Install with: cargo install flamegraph" >&2
      exit 1
    fi
    out="$out_dir/$test_bin-$test_filter.svg"
    cargo flamegraph \
      --output "$out" \
      --no-default-features \
      --profile "$profile" \
      --test "$test_bin" \
      -- "$test_filter" --nocapture
    echo "$out"
    ;;
  *)
    echo "usage: $0 [sample|perf|flamegraph]" >&2
    echo "env: TEST_BIN=$test_bin TEST_FILTER=$test_filter PROFILE=$profile OUT_DIR=$out_dir" >&2
    exit 2
    ;;
esac

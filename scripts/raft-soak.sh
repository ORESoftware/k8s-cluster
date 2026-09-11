#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
cd "$repo_root"

iterations="${SOAK_ITERATIONS:-10}"
duration_seconds="${SOAK_DURATION_SECONDS:-0}"
sleep_seconds="${SOAK_SLEEP_SECONDS:-0}"
out_dir="${SOAK_OUT_DIR:-target/raft-soak}"
gate_mode="${LMX_RAFT_GATE_MODE:-quick}"
run_bench="${RUN_BENCH:-false}"
run_clippy="${RUN_CLIPPY:-true}"
run_secret_scan="${RUN_SECRET_SCAN:-true}"
run_redis_bench="${RUN_REDIS_BENCH:-false}"
run_k8s_raft_live="${RUN_K8S_RAFT_LIVE:-false}"
run_k8s_raft_live_require_metrics="${RUN_K8S_RAFT_LIVE_REQUIRE_METRICS:-true}"
run_raft_version_skew="${RUN_RAFT_VERSION_SKEW:-false}"
test_threads="${TEST_THREADS:-1}"
profile="${PROFILE:-profiling}"
bench_workers="${BENCH_WORKERS:-4}"
bench_keys="${BENCH_KEYS:-128}"
bench_duration_ms="${BENCH_DURATION_MS:-3000}"
redis_host="${REDIS_HOST:-127.0.0.1}"
redis_port="${REDIS_PORT:-17379}"

usage() {
  cat >&2 <<EOF
usage: SOAK_ITERATIONS=10 SOAK_DURATION_SECONDS=0 $0

Runs scripts/raft-hardening-gate.sh repeatedly and stores one log per
iteration plus a TSV summary. A zero duration means use SOAK_ITERATIONS only.
A zero iteration count means run until SOAK_DURATION_SECONDS expires.

env:
  SOAK_ITERATIONS=$iterations
  SOAK_DURATION_SECONDS=$duration_seconds
  SOAK_SLEEP_SECONDS=$sleep_seconds
  SOAK_OUT_DIR=$out_dir
  LMX_RAFT_GATE_MODE=$gate_mode
  RUN_BENCH=$run_bench
  RUN_CLIPPY=$run_clippy
  RUN_SECRET_SCAN=$run_secret_scan
  RUN_REDIS_BENCH=$run_redis_bench
  RUN_K8S_RAFT_LIVE=$run_k8s_raft_live
  RUN_K8S_RAFT_LIVE_REQUIRE_METRICS=$run_k8s_raft_live_require_metrics
  RUN_RAFT_VERSION_SKEW=$run_raft_version_skew
  TEST_THREADS=$test_threads
  PROFILE=$profile
  BENCH_WORKERS=$bench_workers
  BENCH_KEYS=$bench_keys
  BENCH_DURATION_MS=$bench_duration_ms
  REDIS_HOST=$redis_host
  REDIS_PORT=$redis_port
EOF
}

parse_non_negative_int() {
  local name="$1"
  local value="$2"
  if ! [[ "$value" =~ ^[0-9]+$ ]]; then
    echo "$name must be a non-negative integer; got $value" >&2
    exit 2
  fi
}

parse_positive_int() {
  local name="$1"
  local value="$2"
  parse_non_negative_int "$name" "$value"
  if [ "$value" -eq 0 ]; then
    echo "$name must be greater than zero" >&2
    exit 2
  fi
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

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
  usage
  exit 0
fi

parse_non_negative_int SOAK_ITERATIONS "$iterations"
parse_non_negative_int SOAK_DURATION_SECONDS "$duration_seconds"
parse_non_negative_int SOAK_SLEEP_SECONDS "$sleep_seconds"
parse_positive_int TEST_THREADS "$test_threads"
parse_positive_int BENCH_WORKERS "$bench_workers"
parse_positive_int BENCH_KEYS "$bench_keys"
parse_positive_int BENCH_DURATION_MS "$bench_duration_ms"
parse_positive_int REDIS_PORT "$redis_port"

case "$gate_mode" in
  quick | full) ;;
  *)
    echo "LMX_RAFT_GATE_MODE must be quick or full; got $gate_mode" >&2
    exit 2
    ;;
esac

run_bench="$(normalize_bool RUN_BENCH "$run_bench")"
run_clippy="$(normalize_bool RUN_CLIPPY "$run_clippy")"
run_secret_scan="$(normalize_bool RUN_SECRET_SCAN "$run_secret_scan")"
run_redis_bench="$(normalize_bool RUN_REDIS_BENCH "$run_redis_bench")"
run_k8s_raft_live="$(normalize_bool RUN_K8S_RAFT_LIVE "$run_k8s_raft_live")"
run_k8s_raft_live_require_metrics="$(normalize_bool RUN_K8S_RAFT_LIVE_REQUIRE_METRICS "$run_k8s_raft_live_require_metrics")"
run_raft_version_skew="$(normalize_bool RUN_RAFT_VERSION_SKEW "$run_raft_version_skew")"

if [ "$iterations" -eq 0 ] && [ "$duration_seconds" -eq 0 ]; then
  echo "at least one of SOAK_ITERATIONS or SOAK_DURATION_SECONDS must be non-zero" >&2
  exit 2
fi

mkdir -p "$out_dir"
run_started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
safe_run_started_at="${run_started_at//:/-}"
manifest="$out_dir/manifest-${safe_run_started_at}.tsv"
git_status_file="$out_dir/git-status-${safe_run_started_at}.txt"
git_diff_stat_file="$out_dir/git-diff-stat-${safe_run_started_at}.txt"
env_file="$out_dir/env-${safe_run_started_at}.tsv"
summary="$out_dir/summary-${safe_run_started_at}.tsv"
aggregate_summary="$out_dir/summary.tsv"
if [ ! -f "$aggregate_summary" ]; then
  printf 'run_started_at\titeration\tstarted_at\telapsed_seconds\texit_code\tlog\tmanifest\n' >"$aggregate_summary"
fi
if [ ! -f "$summary" ]; then
  printf 'iteration\tstarted_at\telapsed_seconds\texit_code\tlog\n' >"$summary"
fi

capture_command_output() {
  local output_file="$1"
  shift
  if "$@" >"$output_file" 2>&1; then
    return 0
  fi
  printf 'command failed: %s\n' "$*" >>"$output_file"
  return 0
}

git_head="$(git rev-parse HEAD 2>/dev/null || printf 'unknown')"
git_branch="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || printf 'unknown')"
capture_command_output "$git_status_file" git status --short
capture_command_output "$git_diff_stat_file" git diff --stat
{
  printf 'key\tvalue\n'
  printf 'SOAK_ITERATIONS\t%s\n' "$iterations"
  printf 'SOAK_DURATION_SECONDS\t%s\n' "$duration_seconds"
  printf 'SOAK_SLEEP_SECONDS\t%s\n' "$sleep_seconds"
  printf 'SOAK_OUT_DIR\t%s\n' "$out_dir"
  printf 'LMX_RAFT_GATE_MODE\t%s\n' "$gate_mode"
  printf 'RUN_BENCH\t%s\n' "$run_bench"
  printf 'RUN_CLIPPY\t%s\n' "$run_clippy"
  printf 'RUN_SECRET_SCAN\t%s\n' "$run_secret_scan"
  printf 'RUN_REDIS_BENCH\t%s\n' "$run_redis_bench"
  printf 'RUN_K8S_RAFT_LIVE\t%s\n' "$run_k8s_raft_live"
  printf 'RUN_K8S_RAFT_LIVE_REQUIRE_METRICS\t%s\n' "$run_k8s_raft_live_require_metrics"
  printf 'RUN_RAFT_VERSION_SKEW\t%s\n' "$run_raft_version_skew"
  printf 'REDIS_HOST\t%s\n' "$redis_host"
  printf 'REDIS_PORT\t%s\n' "$redis_port"
  printf 'LMX_RAFT_OLD_BIN\t%s\n' "${LMX_RAFT_OLD_BIN:-}"
  printf 'LMX_RAFT_NEW_BIN\t%s\n' "${LMX_RAFT_NEW_BIN:-}"
  printf 'LMX_LIVE_RAFT_HTTP\t%s\n' "${LMX_LIVE_RAFT_HTTP:-}"
  printf 'LMX_LIVE_RAFT_NAMESPACE\t%s\n' "${LMX_LIVE_RAFT_NAMESPACE:-}"
  printf 'LMX_LIVE_RAFT_STATEFULSET\t%s\n' "${LMX_LIVE_RAFT_STATEFULSET:-}"
  printf 'LMX_LIVE_RAFT_KUBECTL_FAILOVER\t%s\n' "${LMX_LIVE_RAFT_KUBECTL_FAILOVER:-}"
  printf 'TEST_THREADS\t%s\n' "$test_threads"
  printf 'PROFILE\t%s\n' "$profile"
  printf 'BENCH_WORKERS\t%s\n' "$bench_workers"
  printf 'BENCH_KEYS\t%s\n' "$bench_keys"
  printf 'BENCH_DURATION_MS\t%s\n' "$bench_duration_ms"
} >"$env_file"
{
  printf 'key\tvalue\n'
  printf 'started_at\t%s\n' "$run_started_at"
  printf 'repo_root\t%s\n' "$repo_root"
  printf 'git_head\t%s\n' "$git_head"
  printf 'git_branch\t%s\n' "$git_branch"
  printf 'git_status_file\t%s\n' "$git_status_file"
  printf 'git_diff_stat_file\t%s\n' "$git_diff_stat_file"
  printf 'env_file\t%s\n' "$env_file"
  printf 'summary_file\t%s\n' "$summary"
  printf 'aggregate_summary_file\t%s\n' "$aggregate_summary"
  printf 'host\t%s\n' "$(hostname 2>/dev/null || printf 'unknown')"
  printf 'uname\t%s\n' "$(uname -a 2>/dev/null || printf 'unknown')"
} >"$manifest"

start_epoch="$(date +%s)"
iteration=0

while :; do
  now="$(date +%s)"
  elapsed_total=$((now - start_epoch))
  if [ "$iteration" -gt 0 ] && [ "$duration_seconds" -gt 0 ] && [ "$elapsed_total" -ge "$duration_seconds" ]; then
    break
  fi
  if [ "$iterations" -gt 0 ] && [ "$iteration" -ge "$iterations" ]; then
    break
  fi

  iteration=$((iteration + 1))
  started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  safe_started_at="${started_at//:/-}"
  log="$out_dir/raft-soak-${safe_started_at}-iter-${iteration}.log"
  iter_start="$(date +%s)"

  printf '\n==> raft soak iteration %s started at %s\n' "$iteration" "$started_at" >&2
  printf '    log: %s\n' "$log" >&2
  set +e
  env \
    LMX_RAFT_GATE_MODE="$gate_mode" \
    RUN_BENCH="$run_bench" \
    RUN_CLIPPY="$run_clippy" \
    RUN_SECRET_SCAN="$run_secret_scan" \
    RUN_REDIS_BENCH="$run_redis_bench" \
    RUN_K8S_RAFT_LIVE="$run_k8s_raft_live" \
    RUN_K8S_RAFT_LIVE_REQUIRE_METRICS="$run_k8s_raft_live_require_metrics" \
    RUN_RAFT_VERSION_SKEW="$run_raft_version_skew" \
    REDIS_HOST="$redis_host" \
    REDIS_PORT="$redis_port" \
    TEST_THREADS="$test_threads" \
    PROFILE="$profile" \
    BENCH_WORKERS="$bench_workers" \
    BENCH_KEYS="$bench_keys" \
    BENCH_DURATION_MS="$bench_duration_ms" \
    "$script_dir/raft-hardening-gate.sh" >"$log" 2>&1
  exit_code=$?
  set -e

  iter_elapsed=$(($(date +%s) - iter_start))
  printf '%s\t%s\t%s\t%s\t%s\n' "$iteration" "$started_at" "$iter_elapsed" "$exit_code" "$log" >>"$summary"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$run_started_at" "$iteration" "$started_at" "$iter_elapsed" "$exit_code" "$log" "$manifest" >>"$aggregate_summary"

  if [ "$exit_code" -ne 0 ]; then
    {
      printf 'failed_at\t%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
      printf 'failed_iteration\t%s\n' "$iteration"
      printf 'failed_exit_code\t%s\n' "$exit_code"
      printf 'failed_log\t%s\n' "$log"
    } >>"$manifest"
    echo "raft soak iteration $iteration failed after ${iter_elapsed}s; tailing $log" >&2
    tail -200 "$log" >&2 || true
    exit "$exit_code"
  fi

  printf '==> raft soak iteration %s passed in %ss\n' "$iteration" "$iter_elapsed" >&2
  if [ "$sleep_seconds" -gt 0 ]; then
    sleep "$sleep_seconds"
  fi
done

total_elapsed=$(($(date +%s) - start_epoch))
{
  printf 'completed_at\t%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  printf 'iterations_completed\t%s\n' "$iteration"
  printf 'total_elapsed_seconds\t%s\n' "$total_elapsed"
} >>"$manifest"
echo "raft soak completed $iteration iteration(s) in ${total_elapsed}s; summary: $summary manifest: $manifest" >&2
